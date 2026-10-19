//! Reserved inline-object slots (TXT-006 R12).
//!
//! Inline objects (icons, formula images, chips) are painted over placeholder characters whose
//! advance equals the object's width plus its gap. The placeholders are recorded as pure
//! insertions in a [`DisplayMap`], so selection, copy and hit testing see the source unchanged.

use crate::display_map::{DisplayMap, Replacement};
use crate::runs::floor_char_boundary;
use std::ops::Range;
use vespertine::TextRun;

/// The placeholder of a slot holding a 12 px object with a 4 px gap in the file-link style: four
/// no-break spaces.
pub const FILE_LINK_SLOT: &str = "\u{a0}\u{a0}\u{a0}\u{a0}";

/// Text with reserved slots inserted.
#[derive(Clone, Debug)]
pub struct SlottedText {
    /// The displayed text, placeholders included.
    pub text: String,
    /// Runs covering the displayed text; slot runs carry no underline or strikethrough.
    pub runs: Vec<TextRun>,
    /// Displayed byte range of every slot, in order.
    pub slots: Vec<Range<usize>>,
    /// Map from displayed offsets back to the source offsets (every slot is a pure insertion).
    pub map: DisplayMap,
}

/// Inserts a placeholder at each `(source index, placeholder)` (sorted by index).
///
/// Each slot run takes the style of the run that follows the insertion point (the first run when
/// the insertion is at the end) with underline and strikethrough removed. `runs` must cover
/// `text`; insertion indices are clamped down to a character boundary.
pub fn insert_slots(text: &str, runs: &[TextRun], insertions: &[(usize, &str)]) -> SlottedText {
    let mut out_text = String::with_capacity(text.len() + insertions.len() * 8);
    let mut out_runs: Vec<TextRun> = Vec::with_capacity(runs.len() + insertions.len() * 2);
    let mut slots = Vec::with_capacity(insertions.len());
    let mut replacements = Vec::with_capacity(insertions.len());

    // Walk the runs, cutting them at insertion points.
    let mut points: Vec<(usize, &str)> = insertions
        .iter()
        .map(|(at, placeholder)| (floor_char_boundary(text, *at), *placeholder))
        .collect();
    points.sort_by_key(|(at, _)| *at);
    let mut next_point = 0;
    let mut run_end = 0;
    for run in runs {
        let run_start = run_end;
        run_end += run.len;
        let mut piece_start = run_start;
        // Insertions that fall inside this run (or at its start) cut it.
        while let Some(&(at, placeholder)) = points.get(next_point) {
            if at >= run_end {
                break;
            }
            if at > piece_start {
                let mut piece = run.clone();
                piece.len = at - piece_start;
                out_runs.push(piece);
                out_text.push_str(&text[piece_start..at]);
                piece_start = at;
            }
            let displayed_start = out_text.len();
            out_text.push_str(placeholder);
            let mut slot_run = run.clone();
            slot_run.len = placeholder.len();
            slot_run.underline = None;
            slot_run.strikethrough = None;
            out_runs.push(slot_run);
            slots.push(displayed_start..out_text.len());
            replacements.push(Replacement::insertion(at, displayed_start..out_text.len()));
            next_point += 1;
        }
        if run_end > piece_start {
            let mut piece = run.clone();
            piece.len = run_end - piece_start;
            out_runs.push(piece);
            out_text.push_str(&text[piece_start..run_end]);
        }
    }
    debug_assert_eq!(run_end, text.len());
    // Insertions at the very end take the style of the first run.
    for &(at, placeholder) in &points[next_point..] {
        debug_assert_eq!(at, text.len());
        let displayed_start = out_text.len();
        out_text.push_str(placeholder);
        let mut slot_run = runs.first().cloned().unwrap_or_else(|| {
            crate::runs::plain_run(0, &vespertine::font(""), vespertine::black())
        });
        slot_run.len = placeholder.len();
        slot_run.underline = None;
        slot_run.strikethrough = None;
        out_runs.push(slot_run);
        slots.push(displayed_start..out_text.len());
        replacements.push(Replacement::insertion(at, displayed_start..out_text.len()));
    }

    SlottedText {
        text: out_text,
        runs: out_runs,
        slots,
        map: DisplayMap::new(replacements),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::{UnderlineStyle, black, font, px, rgb};

    fn underlined(len: usize) -> TextRun {
        let mut run = crate::runs::plain_run(len, &font("Noto Sans"), rgb(0xeeeeee).into());
        run.underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: None,
            wavy: false,
        });
        run
    }

    #[test]
    fn slot_run_has_no_underline_and_follows_the_next_run() {
        let plain = crate::runs::plain_run(5, &font("Noto Sans"), black());
        let runs = vec![underlined(5), plain];
        let slotted = insert_slots("labelrest!", &runs, &[(5, FILE_LINK_SLOT)]);
        assert_eq!(slotted.text, format!("label{FILE_LINK_SLOT}rest!"));
        assert_eq!(
            slotted.runs.iter().map(|r| r.len).sum::<usize>(),
            slotted.text.len()
        );
        let slot = &slotted.slots[0];
        // The slot is its own run, shaped like the following (plain) run and underline-free.
        let mut offset = 0;
        let mut found = false;
        for run in &slotted.runs {
            if offset == slot.start && run.len == slot.len() {
                assert!(run.underline.is_none() && run.strikethrough.is_none());
                assert_eq!(run.color, black());
                found = true;
            }
            offset += run.len;
        }
        assert!(found);
        // The run before it keeps the underline.
        assert!(slotted.runs[0].underline.is_some());
    }

    #[test]
    fn insertion_in_the_middle_of_a_run_splits_it() {
        let runs = vec![underlined(10)];
        let slotted = insert_slots("0123456789", &runs, &[(4, "\u{a0}\u{a0}")]);
        let lens: Vec<usize> = slotted.runs.iter().map(|r| r.len).collect();
        assert_eq!(lens, vec![4, 4, 6]);
        assert!(slotted.runs[0].underline.is_some() && slotted.runs[2].underline.is_some());
        assert!(slotted.runs[1].underline.is_none());
    }

    #[test]
    fn slot_at_the_end_uses_the_first_run_style() {
        let runs = vec![underlined(3)];
        let slotted = insert_slots("abc", &runs, &[(3, FILE_LINK_SLOT)]);
        assert_eq!(slotted.runs.len(), 2);
        assert!(slotted.runs[1].underline.is_none());
        assert_eq!(slotted.slots, vec![3..3 + FILE_LINK_SLOT.len()]);
    }

    #[test]
    fn ranges_rebase_around_the_slot() {
        let runs = vec![underlined(10)];
        let slotted = insert_slots("0123456789", &runs, &[(5, FILE_LINK_SLOT)]);
        let slot_len = FILE_LINK_SLOT.len();
        // Ending at the insertion point: the slot is not included.
        assert_eq!(slotted.map.source_range_to_displayed(0..5), 0..5);
        // Starting at it: the slot's left side is excluded.
        assert_eq!(
            slotted.map.source_range_to_displayed(5..10),
            5 + slot_len..10 + slot_len
        );
        // Selecting across the slot copies the source only.
        let original = crate::display_map::OriginalText::plain("0123456789")
            .present(slotted.text.clone(), slotted.map.clone());
        assert_eq!(original.copy(0..slotted.text.len()), "0123456789");
        assert_eq!(original.copy(5..5 + slot_len), "");
    }
}
