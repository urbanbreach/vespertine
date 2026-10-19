//! Paint-only fade spans for newly arrived text (PRF-017 R3).
//!
//! A [`FadeSpan`] is a range inside a block plus the time the text arrived; its alpha factor is a
//! pure function of its age. Spans only ever turn into colors ([`FadeSpans::color_spans`]):
//! shaping and line breaking depend on text, font and size alone, so applying, advancing or
//! removing spans never reshapes or re-measures anything and never changes layout.

use crate::block_document::BlockId;
use std::{collections::HashMap, ops::Range, time::Duration, time::Instant};
use vespertine::Hsla;

/// Text that arrived at `born`, fading in over the owning set's fade duration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FadeSpan {
    /// The byte range inside the block.
    pub range: Range<usize>,
    /// When the text arrived.
    pub born: Instant,
}

/// The fade spans of one block.
#[derive(Clone, Debug)]
pub struct FadeSpans {
    duration: Duration,
    spans: Vec<FadeSpan>,
}

impl FadeSpans {
    /// An empty set whose spans become fully opaque `duration` after their arrival.
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            spans: Vec::new(),
        }
    }

    /// The fade duration.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Records text at `range` of the block arriving at `now`.
    pub fn arrive(&mut self, range: Range<usize>, now: Instant) {
        if range.start < range.end {
            self.spans.push(FadeSpan { range, born: now });
        }
    }

    /// The alpha factor of `span` at `now`: 0 at arrival rising linearly to 1 after the fade
    /// duration (1 for a zero duration).
    pub fn alpha(&self, span: &FadeSpan, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let age = now.saturating_duration_since(span.born);
        (age.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// Drops the spans that have completed (alpha 1): they paint like ordinary text.
    pub fn advance(&mut self, now: Instant) {
        let duration = self.duration;
        self.spans.retain(|span| {
            duration > Duration::ZERO && now.saturating_duration_since(span.born) < duration
        });
    }

    /// Whether no span is fading.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Removes every span.
    pub fn clear(&mut self) {
        self.spans.clear();
    }

    /// Cuts the spans to a block of `len` bytes (the block shrank).
    pub fn clamp_to(&mut self, len: usize) {
        for span in &mut self.spans {
            span.range.end = span.range.end.min(len);
        }
        self.spans.retain(|span| span.range.start < span.range.end);
    }

    /// The spans with their alpha at `now`.
    pub fn alphas(&self, now: Instant) -> Vec<(Range<usize>, f32)> {
        self.spans
            .iter()
            .map(|span| (span.range.clone(), self.alpha(span, now)))
            .collect()
    }

    /// The spans as `(byte range, color)` pairs ready for run building (`runs_from_spans` of the
    /// rich-text crate): `text` with its alpha multiplied by the span's alpha factor.
    pub fn color_spans(&self, text: Hsla, now: Instant) -> Vec<(Range<usize>, Hsla)> {
        self.spans
            .iter()
            .map(|span| {
                let mut color = text;
                color.a *= self.alpha(span, now);
                (span.range.clone(), color)
            })
            .collect()
    }
}

/// The fade spans of a document, by block identity.
#[derive(Clone, Debug)]
pub struct DocumentFades {
    duration: Duration,
    blocks: HashMap<BlockId, FadeSpans>,
}

impl DocumentFades {
    /// A set whose spans fade in over `duration`.
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            blocks: HashMap::new(),
        }
    }

    /// Records `ranges` (block-relative, as from `BlockDocument::blocks_in`) arriving at `now`.
    pub fn arrive(
        &mut self,
        ranges: impl IntoIterator<Item = (BlockId, Range<usize>)>,
        now: Instant,
    ) {
        for (block, range) in ranges {
            self.blocks
                .entry(block)
                .or_insert_with(|| FadeSpans::new(self.duration))
                .arrive(range, now);
        }
    }

    /// The fade spans of `block`.
    pub fn block(&self, block: BlockId) -> Option<&FadeSpans> {
        self.blocks.get(&block)
    }

    /// Advances every block's spans to `now` and forgets blocks with nothing left fading.
    pub fn advance(&mut self, now: Instant) {
        self.blocks.retain(|_, spans| {
            spans.advance(now);
            !spans.is_empty()
        });
    }

    /// Forgets the spans of a block that no longer exists.
    pub fn remove(&mut self, block: BlockId) {
        self.blocks.remove(&block);
    }

    /// Whether any span is still fading.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::{hsla, rgb};

    #[test]
    fn alpha_rises_linearly_with_age_and_clamps() {
        let start = Instant::now();
        let mut spans = FadeSpans::new(Duration::from_millis(200));
        spans.arrive(0..5, start);
        spans.arrive(5..5, start); // empty ranges are ignored
        let alpha = |at: u64| spans.alpha(&spans.spans[0], start + Duration::from_millis(at));
        assert_eq!(alpha(0), 0.0);
        assert!((alpha(50) - 0.25).abs() < 1e-6);
        assert_eq!(alpha(200), 1.0);
        assert_eq!(alpha(900), 1.0);
        assert_eq!(
            spans.alpha(&spans.spans[0], start - Duration::from_millis(5)),
            0.0
        );
        assert_eq!(spans.spans.len(), 1);
    }

    #[test]
    fn advancing_removes_completed_spans_only() {
        let start = Instant::now();
        let mut spans = FadeSpans::new(Duration::from_millis(100));
        spans.arrive(0..3, start);
        spans.arrive(3..6, start + Duration::from_millis(60));
        spans.advance(start + Duration::from_millis(110));
        assert_eq!(spans.alphas(start + Duration::from_millis(110)).len(), 1);
        assert_eq!(spans.spans[0].range, 3..6);
        spans.advance(start + Duration::from_millis(170));
        assert!(spans.is_empty());
        // A zero duration never fades: nothing is retained.
        let mut instant = FadeSpans::new(Duration::ZERO);
        instant.arrive(0..4, start);
        assert_eq!(instant.alphas(start), vec![(0..4, 1.0)]);
        instant.advance(start);
        assert!(instant.is_empty());
    }

    #[test]
    fn color_spans_scale_the_alpha_of_the_text_color() {
        let start = Instant::now();
        let mut spans = FadeSpans::new(Duration::from_millis(100));
        spans.arrive(2..6, start);
        let text: Hsla = rgb(0x1f2328).into();
        let colors = spans.color_spans(text, start + Duration::from_millis(40));
        assert_eq!(colors.len(), 1);
        assert_eq!(colors[0].0, 2..6);
        assert!((colors[0].1.a - 0.4).abs() < 1e-6);
        assert_eq!(
            (colors[0].1.h, colors[0].1.s, colors[0].1.l),
            (text.h, text.s, text.l)
        );
        let half = hsla(0.5, 0.5, 0.5, 0.5);
        let colors = spans.color_spans(half, start + Duration::from_millis(40));
        assert!((colors[0].1.a - 0.2).abs() < 1e-6);
    }

    #[test]
    fn clamping_follows_a_shrinking_block_and_the_document_set_forgets_settled_blocks() {
        let start = Instant::now();
        let mut spans = FadeSpans::new(Duration::from_millis(100));
        spans.arrive(0..4, start);
        spans.arrive(8..12, start);
        spans.clamp_to(10);
        assert_eq!(
            spans
                .alphas(start)
                .iter()
                .map(|(r, _)| r.clone())
                .collect::<Vec<_>>(),
            vec![0..4, 8..10]
        );
        spans.clamp_to(2);
        assert_eq!(spans.alphas(start).len(), 1);

        let mut fades = DocumentFades::new(Duration::from_millis(100));
        fades.arrive([(BlockId(1), 0..3), (BlockId(2), 0..5)], start);
        assert!(fades.block(BlockId(2)).is_some());
        fades.remove(BlockId(2));
        assert!(fades.block(BlockId(2)).is_none());
        fades.advance(start + Duration::from_millis(50));
        assert!(!fades.is_empty());
        fades.advance(start + Duration::from_millis(150));
        assert!(fades.is_empty());
    }
}
