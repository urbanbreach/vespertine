//! Grapheme-safe shortened labels (TXT-006 R13).

use crate::display_map::{DisplayMap, Replacement};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use vespertine::Pixels;

/// The ellipsis used for shortened labels.
pub const ELLIPSIS: &str = "\u{2026}";

/// Omissions of `ELLIPSIS.len()` bytes or fewer are not recorded: a short label is never made
/// longer.
pub const ELLIPSIS_BYTES: usize = ELLIPSIS.len();

/// A text with styled runs and the ranges that label fitting works on.
///
/// `S` is the caller's run style (a font and decorations); the helper never inspects it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatText<S> {
    /// The displayed text. Inline-object slots are already present as placeholder characters.
    pub text: String,
    /// Styled runs as `(byte length, style)`, covering `text` exactly.
    pub runs: Vec<(usize, S)>,
    /// Link ranges; each one is a label that is shortened on its own.
    pub links: Vec<Range<usize>>,
    /// Code-span ranges.
    pub code: Vec<Range<usize>>,
    /// Atomic inline-object slot ranges: shown whole or replaced by the ellipsis, never cut.
    pub slots: Vec<Range<usize>>,
}

/// A shortened flat text and the map from its text back to the text that was fitted.
#[derive(Clone, Debug)]
pub struct Shortened<S> {
    /// The shortened text with re-expressed runs and ranges.
    pub flat: FlatText<S>,
    /// Displayed (shortened) offsets to the fitted text's offsets.
    pub map: DisplayMap,
}

fn is_slot_run(run: &Range<usize>, slots: &[Range<usize>]) -> bool {
    slots
        .iter()
        .any(|slot| slot.start <= run.start && run.end <= slot.end && run.start < run.end)
}

/// Runs clipped to `range`, as `(length, style)`.
fn clipped_runs<S: Clone>(runs: &[(usize, S)], range: Range<usize>) -> Vec<(usize, S)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for (len, style) in runs {
        let (start, end) = (offset.max(range.start), (offset + len).min(range.end));
        if end > start {
            out.push((end - start, style.clone()));
        }
        offset += len;
    }
    out
}

/// Style of the first run overlapping `range` that is not a slot run.
fn first_non_slot_style<S: Clone>(
    runs: &[(usize, S)],
    range: Range<usize>,
    slots: &[Range<usize>],
) -> Option<S> {
    let mut offset = 0;
    for (len, style) in runs {
        let run = offset..offset + len;
        offset += len;
        if run.end <= range.start || run.start >= range.end {
            continue;
        }
        if !is_slot_run(&run, slots) {
            return Some(style.clone());
        }
    }
    None
}

/// Shortens every link label of `flat` that is wider than `width`.
///
/// For each label: if it measures no wider than `width` it is left alone. Otherwise the greatest
/// grapheme boundary `b` (never strictly inside a slot) such that `label[..b]` followed by U+2026
/// measures no wider than `width` is found by binary search (`b = 0` when not even the empty
/// prefix fits). The kept part is measured with the label's own run styles and the ellipsis with
/// the style of the first omitted non-slot run. The omission `[label.start + b, label.end)` is
/// replaced by the ellipsis only when more than [`ELLIPSIS_BYTES`] source bytes are removed. Link,
/// code and slot ranges are re-expressed through the resulting map; slots inside an omitted tail
/// are dropped.
///
/// `measure(text, runs)` returns the shaped width of `text` with `runs` (lengths in bytes).
pub fn fit_labels<S: Clone>(
    flat: &FlatText<S>,
    width: Pixels,
    mut measure: impl FnMut(&str, &[(usize, S)]) -> Pixels,
) -> Shortened<S> {
    // (source range of the omitted tail, style of the ellipsis)
    let mut omissions: Vec<(Range<usize>, S)> = Vec::new();
    let mut links = flat.links.clone();
    links.sort_by_key(|link| (link.start, link.end));
    for link in &links {
        let (start, end) = (
            crate::runs::floor_char_boundary(&flat.text, link.start),
            crate::runs::floor_char_boundary(&flat.text, link.end),
        );
        if end <= start || omissions.last().is_some_and(|(tail, _)| start < tail.end) {
            continue;
        }
        let label = &flat.text[start..end];
        let label_runs = clipped_runs(&flat.runs, start..end);
        if measure(label, &label_runs) <= width {
            continue;
        }
        let mut boundaries: Vec<usize> = label
            .grapheme_indices(true)
            .map(|(ix, _)| ix)
            .filter(|ix| {
                !flat
                    .slots
                    .iter()
                    .any(|slot| slot.start < start + ix && start + ix < slot.end)
            })
            .collect();
        if boundaries.first() != Some(&0) {
            boundaries.insert(0, 0);
        }
        let ellipsis_style = |b: usize| {
            first_non_slot_style(&flat.runs, start + b..end, &flat.slots)
                .or_else(|| flat.runs.first().map(|(_, style)| style.clone()))
        };
        let mut fits = |b: usize| {
            let Some(style) = ellipsis_style(b) else {
                return false;
            };
            let mut text = String::with_capacity(b + ELLIPSIS_BYTES);
            text.push_str(&label[..b]);
            text.push_str(ELLIPSIS);
            let mut runs = clipped_runs(&flat.runs, start..start + b);
            runs.push((ELLIPSIS_BYTES, style));
            measure(&text, &runs) <= width
        };
        // Greatest boundary that fits; the predicate is monotone in the boundary.
        let (mut lo, mut hi) = (0, boundaries.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if fits(boundaries[mid]) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let b = if lo == 0 { 0 } else { boundaries[lo - 1] };
        if end - (start + b) > ELLIPSIS_BYTES
            && let Some(style) = ellipsis_style(b)
        {
            omissions.push((start + b..end, style));
        }
    }

    if omissions.is_empty() {
        return Shortened {
            flat: flat.clone(),
            map: DisplayMap::identity(),
        };
    }

    let mut text = String::with_capacity(flat.text.len());
    let mut runs: Vec<(usize, S)> = Vec::new();
    let mut replacements = Vec::with_capacity(omissions.len());
    let mut cursor = 0;
    for (tail, style) in &omissions {
        text.push_str(&flat.text[cursor..tail.start]);
        runs.extend(clipped_runs(&flat.runs, cursor..tail.start));
        let displayed_start = text.len();
        text.push_str(ELLIPSIS);
        runs.push((ELLIPSIS_BYTES, style.clone()));
        replacements.push(Replacement::new(
            tail.clone(),
            displayed_start..displayed_start + ELLIPSIS_BYTES,
        ));
        cursor = tail.end;
    }
    text.push_str(&flat.text[cursor..]);
    runs.extend(clipped_runs(&flat.runs, cursor..flat.text.len()));
    let map = DisplayMap::new(replacements);

    let omitted = |range: &Range<usize>| {
        omissions
            .iter()
            .any(|(tail, _)| tail.start <= range.start && range.end <= tail.end)
    };
    let express = |ranges: &[Range<usize>], drop_omitted: bool| -> Vec<Range<usize>> {
        ranges
            .iter()
            .filter(|range| !(drop_omitted && omitted(range)))
            .map(|range| map.source_range_to_displayed(range.clone()))
            .collect()
    };
    Shortened {
        flat: FlatText {
            links: express(&flat.links, false),
            code: express(&flat.code, false),
            slots: express(&flat.slots, true),
            text,
            runs,
        },
        map,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::px;

    /// 10 px per char (grapheme-agnostic: counts chars), the ellipsis counts as one char.
    fn measure(text: &str, _runs: &[(usize, ())]) -> Pixels {
        px(text.chars().count() as f32 * 10.0)
    }

    fn flat(text: &str, link: Range<usize>) -> FlatText<()> {
        FlatText {
            text: text.to_string(),
            runs: vec![(text.len(), ())],
            links: vec![link],
            code: vec![],
            slots: vec![],
        }
    }

    #[test]
    fn short_label_is_unchanged() {
        let flat = flat("see label here", 4..9);
        let result = fit_labels(&flat, px(100.), measure);
        assert_eq!(result.flat, flat);
        assert!(result.map.is_identity());
    }

    #[test]
    fn long_label_is_cut_on_a_grapheme_boundary_with_ellipsis() {
        // 'e' + combining acute, a CJK character, a ZWJ emoji family, then ascii.
        let label = "e\u{301}\u{4e2d}\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}abcdefghijkl";
        let text = format!("go {label} now");
        let link = 3..3 + label.len();
        let flat = flat(&text, link.clone());
        // Room for 5 chars: the kept prefix plus the ellipsis must fit in 50 px.
        let result = fit_labels(&flat, px(50.), measure);
        assert!(result.flat.text.contains(ELLIPSIS));
        let ellipsis_at = result.flat.text.find(ELLIPSIS).unwrap();
        // The cut is on a grapheme boundary of the original label.
        let cut = result.map.displayed_to_source(ellipsis_at);
        assert!(
            label.grapheme_indices(true).any(|(ix, _)| 3 + ix == cut),
            "cut {cut} not on a grapheme boundary"
        );
        assert!(text.is_char_boundary(cut));
        assert!(measure(&result.flat.text[link.start..ellipsis_at + 3], &[]) <= px(50.));
        // The link range ends right after the ellipsis.
        assert_eq!(result.flat.links[0].end, ellipsis_at + 3);
        // Copying the whole displayed text gives the full source.
        let original = crate::display_map::OriginalText::plain(text.clone())
            .present(result.flat.text.clone(), result.map.clone());
        assert_eq!(original.copy(0..result.flat.text.len()), text);
    }

    #[test]
    fn label_that_would_lose_only_three_bytes_is_left_alone() {
        // "abcdefghijk" needs 110 px. With room for 9 chars the cut keeps "abcdefgh" and would
        // omit exactly 3 bytes, the size of the ellipsis, so the label stays as it is.
        let label = flat("abcdefghijk", 0..11);
        let result = fit_labels(&label, px(90.), measure);
        assert_eq!(result.flat.text, "abcdefghijk");
        assert!(result.map.is_identity());
        // Losing 4 bytes (more than the ellipsis) is recorded.
        let longer = flat("abcdefghijkl", 0..12);
        let result = fit_labels(&longer, px(90.), measure);
        assert_eq!(result.flat.text, "abcdefgh\u{2026}");
    }

    #[test]
    fn cut_never_lands_inside_a_slot() {
        // Link "ab" + four nbsp slot placeholders at the end, as for file-style links.
        let text = "ab\u{a0}\u{a0}\u{a0}\u{a0}";
        let mut label = flat(text, 0..text.len());
        label.slots = vec![2..text.len()];
        for width in 0..8 {
            let result = fit_labels(&label, px(width as f32 * 10.0), measure);
            if let Some(replacement) = result.map.replacements().first() {
                let cut = replacement.source.start;
                assert!(
                    !(2 < cut && cut < text.len()),
                    "cut {cut} inside slot at width {width}"
                );
            }
        }
    }

    #[test]
    fn slots_in_an_omitted_tail_are_dropped_and_ranges_re_expressed() {
        let text = "xx abcdefghijklmnop\u{a0}\u{a0}\u{a0}\u{a0} yy";
        let slot_start = "xx abcdefghijklmnop".len();
        let mut label = flat(text, 3..slot_start + 8);
        label.slots = vec![slot_start..slot_start + 8];
        label.code = vec![0..2];
        let result = fit_labels(&label, px(60.), measure);
        assert!(result.flat.slots.is_empty());
        assert_eq!(result.flat.code, vec![0..2]);
        assert!(result.flat.text.ends_with(" yy"));
        assert!(result.flat.text.contains(ELLIPSIS));
    }
}
