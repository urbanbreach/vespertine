//! Word-level change intervals of the native viewer (CMP-015 R13).
//!
//! Within a hunk a maximal run of deleted lines immediately followed by a maximal run of added
//! lines is paired index by index, but only when both runs have the same non-zero count:
//! pairing unequal runs by index would produce confident, wrong highlights. For a pair the
//! lines are tokenized, the longest equal token prefix and then suffix are trimmed, and what
//! remains on each side is one contiguous byte interval.

use super::model::{DiffLine, LineKind};
use std::ops::Range;

/// Tokens of a line as byte ranges: a maximal run of alphanumeric characters and underscores is
/// one token, every other character is its own token.
fn tokens(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut word_start: Option<usize> = None;
    for (index, c) in text.char_indices() {
        if c.is_alphanumeric() || c == '_' {
            word_start.get_or_insert(index);
        } else {
            if let Some(start) = word_start.take() {
                out.push(start..index);
            }
            out.push(index..index + c.len_utf8());
        }
    }
    if let Some(start) = word_start {
        out.push(start..text.len());
    }
    out
}

/// The changed byte intervals of an old/new pair: `(old interval, new interval)`. An empty
/// remainder (a pure insertion or deletion, or identical lines) gives `None` on that side.
pub fn pair_intervals(old: &str, new: &str) -> (Option<Range<usize>>, Option<Range<usize>>) {
    let old_tokens = tokens(old);
    let new_tokens = tokens(new);
    let same = |a: &Range<usize>, b: &Range<usize>| old[a.clone()] == new[b.clone()];
    let limit = old_tokens.len().min(new_tokens.len());
    let mut prefix = 0;
    while prefix < limit && same(&old_tokens[prefix], &new_tokens[prefix]) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < limit - prefix
        && same(
            &old_tokens[old_tokens.len() - 1 - suffix],
            &new_tokens[new_tokens.len() - 1 - suffix],
        )
    {
        suffix += 1;
    }
    let interval = |tokens: &[Range<usize>]| {
        let middle = &tokens[prefix..tokens.len() - suffix];
        Some(middle.first()?.start..middle.last()?.end)
    };
    (interval(&old_tokens), interval(&new_tokens))
}

/// Word-change interval of every line of a hunk (byte range in the line's text), `None` for
/// lines without a wash.
pub fn hunk_intervals(lines: &[DiffLine]) -> Vec<Option<Range<usize>>> {
    let mut out = vec![None; lines.len()];
    let mut index = 0;
    while index < lines.len() {
        if lines[index].kind != LineKind::Deleted {
            index += 1;
            continue;
        }
        let deleted_start = index;
        while index < lines.len() && lines[index].kind == LineKind::Deleted {
            index += 1;
        }
        let added_start = index;
        while index < lines.len() && lines[index].kind == LineKind::Added {
            index += 1;
        }
        let deleted = added_start - deleted_start;
        let added = index - added_start;
        if deleted == 0 || deleted != added {
            continue;
        }
        for offset in 0..deleted {
            let (old, new) = pair_intervals(
                &lines[deleted_start + offset].text,
                &lines[added_start + offset].text,
            );
            out[deleted_start + offset] = old;
            out[added_start + offset] = new;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            text: text.into(),
            old: None,
            new: None,
        }
    }

    fn interval(text: &str, range: Option<Range<usize>>) -> Option<&str> {
        range.map(|range| &text[range])
    }

    #[test]
    fn changing_one_identifier_in_a_call_changes_exactly_one_token_interval() {
        let old = "let x = render(frame_count, 3);";
        let new = "let x = render(frame_total, 3);";
        let (a, b) = pair_intervals(old, new);
        assert_eq!(interval(old, a), Some("frame_count"));
        assert_eq!(interval(new, b), Some("frame_total"));
    }

    #[test]
    fn prefix_then_suffix_are_trimmed_without_overlapping() {
        let (a, b) = pair_intervals("aXb", "aXXb");
        // tokens: aXb | aXXb are single word tokens, so the whole word changes.
        assert_eq!((a, b), (Some(0..3), Some(0..4)));
        let (a, b) = pair_intervals("a b", "a b b");
        assert_eq!(a, None, "pure insertion has no wash on the old side");
        assert_eq!(b, Some(3..5), "inserted ` b` tokens");
        let (a, b) = pair_intervals("x, y", "x, y");
        assert_eq!((a, b), (None, None), "identical lines");
        // Prefix and suffix would overlap on repeated tokens; the suffix stops at the prefix.
        let (a, b) = pair_intervals("..", "...");
        assert_eq!(a, None);
        assert_eq!(b, Some(2..3));
    }

    #[test]
    fn punctuation_and_other_characters_are_single_tokens() {
        let (a, b) = pair_intervals("f(a)", "f[a]");
        assert_eq!(interval("f(a)", a), Some("(a)"));
        assert_eq!(interval("f[a]", b), Some("[a]"));
        let (a, _) = pair_intervals("caf\u{e9} x", "cafe x");
        assert_eq!(
            interval("caf\u{e9} x", a),
            Some("caf\u{e9}"),
            "alphanumeric run"
        );
        let (a, b) = pair_intervals("a\u{4e2d}b", "a\u{6587}b");
        assert_eq!(
            interval("a\u{4e2d}b", a),
            Some("a\u{4e2d}b"),
            "CJK is alphanumeric"
        );
        let _ = b;
    }

    #[test]
    fn only_equal_count_runs_are_paired() {
        let lines = [
            line(LineKind::Deleted, "one a"),
            line(LineKind::Deleted, "two a"),
            line(LineKind::Added, "one b"),
            line(LineKind::Added, "two b"),
        ];
        let intervals = hunk_intervals(&lines);
        assert_eq!(interval("one a", intervals[0].clone()), Some("a"));
        assert_eq!(interval("two b", intervals[3].clone()), Some("b"));
        let unequal = [
            line(LineKind::Deleted, "one a"),
            line(LineKind::Deleted, "two a"),
            line(LineKind::Deleted, "three a"),
            line(LineKind::Added, "one b"),
        ];
        assert!(hunk_intervals(&unequal).iter().all(Option::is_none));
        let added_only = [line(LineKind::Added, "x"), line(LineKind::Added, "y")];
        assert!(hunk_intervals(&added_only).iter().all(Option::is_none));
    }

    #[test]
    fn a_meta_line_ends_a_run() {
        let lines = [
            line(LineKind::Deleted, "a x"),
            line(LineKind::Meta, "No newline at end of file"),
            line(LineKind::Added, "a y"),
        ];
        assert!(hunk_intervals(&lines).iter().all(Option::is_none));
        let lines = [
            line(LineKind::Context, "same"),
            line(LineKind::Deleted, "a x"),
            line(LineKind::Added, "a y"),
            line(LineKind::Context, "same"),
            line(LineKind::Deleted, "b x"),
            line(LineKind::Added, "b y"),
        ];
        let intervals = hunk_intervals(&lines);
        assert_eq!(intervals[0], None);
        assert_eq!(interval("a x", intervals[1].clone()), Some("x"));
        assert_eq!(interval("b y", intervals[5].clone()), Some("y"));
    }
}
