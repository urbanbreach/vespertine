//! Ordered source/display offset mapping (TXT-006 R8-R12).
//!
//! Displayed text may differ from the source it presents: a shortened label shows one ellipsis for
//! many source bytes, a reserved inline-object slot shows placeholder characters for no source at
//! all, and break opportunities are display-only characters. A [`DisplayMap`] records those
//! replacements so hit testing, selection and copy resolve to the original source.

use std::ops::Range;
use std::sync::Arc;

/// One replacement: the `source` range is shown as the `displayed` range.
///
/// The displayed range may be shorter than the source range (a shortened label) or longer (a
/// reserved slot); a source range may be empty (a pure insertion).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    /// Byte range in the stage's source text.
    pub source: Range<usize>,
    /// Byte range in the stage's displayed text.
    pub displayed: Range<usize>,
}

impl Replacement {
    /// A replacement of `source` by `displayed`.
    pub fn new(source: Range<usize>, displayed: Range<usize>) -> Self {
        Self { source, displayed }
    }

    /// A pure insertion of `displayed.len()` display bytes at source offset `at`.
    pub fn insertion(at: usize, displayed: Range<usize>) -> Self {
        Self {
            source: at..at,
            displayed,
        }
    }
}

/// An ordered list of replacements, optionally chained to the stage closer to the source.
///
/// Replacements are in increasing order and never overlap. With a `prior` stage the source of this
/// stage is the displayed text of `prior`, and mapping to the original source goes through every
/// stage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayMap {
    replacements: Vec<Replacement>,
    prior: Option<Arc<DisplayMap>>,
}

impl DisplayMap {
    /// A map without replacements: displayed and source offsets coincide.
    pub fn identity() -> Self {
        Self::default()
    }

    /// A single-stage map from ordered, non-overlapping replacements.
    pub fn new(replacements: Vec<Replacement>) -> Self {
        debug_assert!(
            replacements.windows(2).all(|pair| {
                pair[0].source.end <= pair[1].source.start
                    && pair[0].displayed.end <= pair[1].displayed.start
            }),
            "display replacements must be ordered and must not overlap: {replacements:?}"
        );
        Self {
            replacements,
            prior: None,
        }
    }

    /// Chains this stage on top of `prior`, the stage closer to the source.
    pub fn with_prior(mut self, prior: DisplayMap) -> Self {
        self.prior = Some(Arc::new(prior));
        self
    }

    /// The replacements of this stage only.
    pub fn replacements(&self) -> &[Replacement] {
        &self.replacements
    }

    /// The stage closer to the source, if any.
    pub fn prior(&self) -> Option<&DisplayMap> {
        self.prior.as_deref()
    }

    /// Whether neither this stage nor any prior stage replaces anything.
    pub fn is_identity(&self) -> bool {
        self.replacements.is_empty() && self.prior.as_ref().is_none_or(|prior| prior.is_identity())
    }

    /// Maps a displayed offset to the source offset, applying this stage first and then the prior
    /// stage's displayed-to-source mapping on the result (R9, R10).
    pub fn displayed_to_source(&self, displayed: usize) -> usize {
        let here = self.stage_displayed_to_source(displayed);
        match &self.prior {
            Some(prior) => prior.displayed_to_source(here),
            None => here,
        }
    }

    /// Maps a source offset to the displayed offset, applying the prior stage first (R10).
    ///
    /// A source offset that equals an insertion point maps past the inserted slot (R12: a range
    /// that starts at an insertion point excludes the slot's left side).
    pub fn source_to_displayed(&self, source: usize) -> usize {
        let prior = match &self.prior {
            Some(prior) => prior.source_to_displayed(source),
            None => source,
        };
        self.stage_source_to_displayed(prior, false)
    }

    /// Maps a source range to a displayed range (R9, R12).
    ///
    /// Both endpoints are mapped and the result is widened to cover every replacement the range
    /// overlaps. The start shifts past insertions at or before it; the end does not include an
    /// insertion that sits exactly at it.
    pub fn source_range_to_displayed(&self, range: Range<usize>) -> Range<usize> {
        let prior = match &self.prior {
            Some(prior) => prior.source_range_to_displayed(range),
            None => range,
        };
        self.stage_range_to_displayed(prior)
    }

    /// The source range that a displayed range copies: both endpoints map to the source.
    ///
    /// Selecting from inside a shortened label starts at the label's source start; selecting up to
    /// and including it ends at the label's source end.
    pub fn displayed_range_to_source(&self, range: Range<usize>) -> Range<usize> {
        let start = self.displayed_to_source(range.start);
        let end = self.displayed_to_source(range.end).max(start);
        start..end
    }

    fn stage_displayed_to_source(&self, displayed: usize) -> usize {
        // The scan stops at the first replacement that starts after `displayed`; only the last
        // passed replacement matters (its shift already contains every earlier one).
        let passed = self
            .replacements
            .partition_point(|replacement| replacement.displayed.start <= displayed);
        match passed.checked_sub(1).map(|ix| &self.replacements[ix]) {
            None => displayed,
            Some(replacement) if displayed < replacement.displayed.end => replacement.source.start,
            Some(replacement) => {
                (displayed + replacement.source.end).saturating_sub(replacement.displayed.end)
            }
        }
    }

    fn stage_source_to_displayed(&self, source: usize, is_range_end: bool) -> usize {
        let passed = self.replacements.partition_point(|replacement| {
            // An insertion exactly at a range end is not passed: the range does not include it.
            !(source < replacement.source.start
                || (is_range_end
                    && replacement.source.is_empty()
                    && source == replacement.source.start))
        });
        match passed.checked_sub(1).map(|ix| &self.replacements[ix]) {
            None => source,
            Some(replacement) if source < replacement.source.end => replacement.displayed.start,
            Some(replacement) => {
                (source + replacement.displayed.end).saturating_sub(replacement.source.end)
            }
        }
    }

    fn stage_range_to_displayed(&self, range: Range<usize>) -> Range<usize> {
        let mut start = self.stage_source_to_displayed(range.start, false);
        let mut end = self.stage_source_to_displayed(range.end, true);
        for replacement in &self.replacements {
            if range.start < replacement.source.end && range.end > replacement.source.start {
                start = start.min(replacement.displayed.start);
                end = end.max(replacement.displayed.end);
            }
        }
        start..end.max(start)
    }
}

/// Source text together with the (possibly chained) map of the text displayed for it.
///
/// Later presentation stages are added with [`OriginalText::present`]; offsets always resolve to
/// the very first source.
#[derive(Clone, Debug)]
pub struct OriginalText {
    source: Arc<str>,
    displayed: Arc<str>,
    map: DisplayMap,
}

impl OriginalText {
    /// Text that is displayed exactly as written.
    pub fn plain(source: impl Into<Arc<str>>) -> Self {
        let source: Arc<str> = source.into();
        Self {
            displayed: source.clone(),
            source,
            map: DisplayMap::identity(),
        }
    }

    /// Adds a presentation stage: `displayed` is what the current displayed text becomes, and
    /// `stage` maps the current displayed text (its source) to it.
    pub fn present(&self, displayed: impl Into<Arc<str>>, stage: DisplayMap) -> Self {
        let map = if self.map.is_identity() {
            stage
        } else {
            stage.with_prior(self.map.clone())
        };
        Self {
            source: self.source.clone(),
            displayed: displayed.into(),
            map,
        }
    }

    /// The original text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The text as displayed.
    pub fn displayed(&self) -> &str {
        &self.displayed
    }

    /// The chained map.
    pub fn map(&self) -> &DisplayMap {
        &self.map
    }

    /// The source index for a displayed index, clamped to the source and to a character boundary.
    pub fn source_index(&self, displayed: usize) -> usize {
        let displayed = crate::runs::floor_char_boundary(&self.displayed, displayed);
        crate::runs::floor_char_boundary(&self.source, self.map.displayed_to_source(displayed))
    }

    /// The source text that copying a displayed range yields: never the ellipsis, slot
    /// placeholders or display-only break opportunities.
    pub fn copy(&self, displayed: Range<usize>) -> &str {
        let start = crate::runs::floor_char_boundary(&self.displayed, displayed.start);
        let end = crate::runs::floor_char_boundary(&self.displayed, displayed.end).max(start);
        let range = self.map.displayed_range_to_source(start..end);
        let start = crate::runs::floor_char_boundary(&self.source, range.start);
        let end = crate::runs::floor_char_boundary(&self.source, range.end).max(start);
        &self.source[start..end]
    }
}

/// Inserts a zero-width space between the grapheme clusters of `text` so that a long unbroken
/// string (an address) can wrap anywhere; returns the displayed string and the map back to `text`.
pub fn with_break_opportunities(text: &str) -> (String, DisplayMap) {
    use unicode_segmentation::UnicodeSegmentation;
    const ZWSP: &str = "\u{200B}";
    let mut displayed = String::with_capacity(text.len() * 2);
    let mut replacements = Vec::new();
    for (ix, grapheme) in text.grapheme_indices(true) {
        if ix > 0 {
            let start = displayed.len();
            displayed.push_str(ZWSP);
            replacements.push(Replacement::insertion(ix, start..displayed.len()));
        }
        displayed.push_str(grapheme);
    }
    (displayed, DisplayMap::new(replacements))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortened_label_maps_to_source_start_and_end() {
        let map = DisplayMap::new(vec![Replacement::new(10..40, 10..11)]);
        assert_eq!(map.displayed_to_source(10), 10);
        assert_eq!(map.displayed_to_source(11), 40);
        assert_eq!(map.displayed_to_source(5), 5);
        assert_eq!(map.displayed_to_source(20), 49);
        assert_eq!(map.source_to_displayed(10), 10);
        assert_eq!(map.source_to_displayed(25), 10);
        assert_eq!(map.source_to_displayed(40), 11);
        assert_eq!(map.source_to_displayed(45), 16);
    }

    #[test]
    fn chained_stages_resolve_to_the_first_source() {
        // Stage 1 (closer to the source): a 4-byte slot inserted at source 20.
        let slot = DisplayMap::new(vec![Replacement::insertion(20, 20..24)]);
        // Stage 2: the slotted text shortens 30..60 to a 1-byte ellipsis at 30..31.
        let truncation = DisplayMap::new(vec![Replacement::new(30..60, 30..31)]).with_prior(slot);
        // Displayed 31 is the source (slotted) offset 60, which is original offset 56.
        assert_eq!(truncation.displayed_to_source(31), 56);
        assert_eq!(truncation.displayed_to_source(30), 26);
        assert_eq!(truncation.displayed_to_source(22), 20);
        assert_eq!(truncation.displayed_to_source(24), 20);
        assert_eq!(truncation.displayed_to_source(25), 21);
        assert_eq!(truncation.source_to_displayed(56), 31);
        assert_eq!(truncation.source_to_displayed(26), 30);
        assert_eq!(truncation.source_to_displayed(20), 24);
    }

    #[test]
    fn source_range_overlapping_a_replacement_is_widened() {
        let map = DisplayMap::new(vec![Replacement::new(10..40, 10..11)]);
        assert_eq!(map.source_range_to_displayed(5..20), 5..11);
        assert_eq!(map.source_range_to_displayed(20..50), 10..21);
        assert_eq!(map.source_range_to_displayed(0..5), 0..5);
        assert_eq!(map.source_range_to_displayed(40..45), 11..16);
    }

    #[test]
    fn insertion_points_exclude_the_slot_from_ranges_that_end_there() {
        let map = DisplayMap::new(vec![Replacement::insertion(20, 20..24)]);
        // Ends at the insertion point: the slot is not included.
        assert_eq!(map.source_range_to_displayed(10..20), 10..20);
        // Starts at the insertion point: the slot's left side is excluded.
        assert_eq!(map.source_range_to_displayed(20..30), 24..34);
        // Strictly contains it: the slot is covered.
        assert_eq!(map.source_range_to_displayed(10..30), 10..34);
    }

    #[test]
    fn break_opportunities_never_enter_copied_text() {
        let address = "0xabcdef0123456789";
        let (displayed, map) = with_break_opportunities(address);
        assert!(displayed.contains('\u{200B}'));
        let text = OriginalText::plain(address).present(displayed.clone(), map);
        assert_eq!(text.copy(0..displayed.len()), address);
        assert_eq!(text.copy(0..text.displayed().len()), address);
        let mid = displayed.len() / 2;
        let mid = crate::runs::floor_char_boundary(&displayed, mid);
        assert!(!text.copy(mid..displayed.len()).contains('\u{200B}'));
    }

    #[test]
    fn copy_over_a_shortened_label_returns_the_full_source() {
        let source = "see https://example.com/a/very/long/path for details";
        let range = 4..39;
        let displayed = format!("{}\u{2026}{}", &source[..10], &source[range.end..]);
        let ellipsis_start = 10;
        let map = DisplayMap::new(vec![Replacement::new(
            10..range.end,
            ellipsis_start..ellipsis_start + 3,
        )]);
        let text = OriginalText::plain(source).present(displayed.clone(), map);
        assert_eq!(text.copy(0..displayed.len()), source);
        // Selecting only the ellipsis cell and what follows ends at the label's source end.
        assert_eq!(text.copy(10..displayed.len()), &source[10..]);
        // A click inside the ellipsis maps to the replacement's source start.
        assert_eq!(text.source_index(11), 10);
    }
}
