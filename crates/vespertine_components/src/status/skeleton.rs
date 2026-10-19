//! Skeleton placeholders (CMP-023 R2, R3): non-interactive bars shaped like the content they
//! stand for, pulsing from the shared clock.

use vespertine::{
    App, ElementId, IntoElement, ParentElement, RenderOnce, Role, SharedString, StyleRefinement,
    Styled, Window, div, px, relative,
};
use vespertine::{InteractiveElement as _, StatefulInteractiveElement as _};

use super::indicators::{IndicatorKind, IndicatorView};
use super::motion::{SKELETON_TINT, SLOW_PERIOD, ladder_width, skeleton_opacity};
use super::theme::{StatusColors, pulse, with_alpha};

/// The width of a skeleton bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BarWidth {
    /// A fixed width in logical pixels (never wider than the container).
    Px(f32),
    /// A fraction of the container.
    Fraction(f32),
}

/// One skeleton bar. Bars are not focusable, are excluded from tab order, have no handlers and
/// are hidden from the accessibility tree; the owning block exposes the one progress node.
#[derive(IntoElement)]
pub struct SkeletonBar {
    width: BarWidth,
    height: f32,
    index: usize,
    phase: Option<f32>,
}

/// A skeleton bar of `height` at position `index` of its column (the pulse travels down a
/// column by 0.08 of the period per index). Its width is at most 100% of its container, it
/// does not shrink or grow in flex layouts and has fully rounded ends.
pub fn skeleton_bar(width: BarWidth, height: f32, index: usize) -> SkeletonBar {
    SkeletonBar {
        width,
        height,
        index,
        phase: None,
    }
}

impl SkeletonBar {
    /// Uses an explicit shared-clock phase instead of reading the clock (for blocks that read
    /// it once).
    pub fn phase(mut self, phase: f32) -> Self {
        self.phase = Some(phase);
        self
    }
}

impl RenderOnce for SkeletonBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let phase = match self.phase {
            Some(phase) => phase,
            None => pulse(window, cx, SLOW_PERIOD, false).phase,
        };
        let colors = StatusColors::active(cx);
        let bar = div()
            .h(px(self.height))
            .max_w(relative(1.0))
            .flex_none()
            .rounded(px(self.height / 2.0))
            .bg(with_alpha(colors.text, SKELETON_TINT))
            .opacity(skeleton_opacity(phase, self.index));
        match self.width {
            BarWidth::Px(width) => bar.w(px(width)),
            BarWidth::Fraction(fraction) => bar.w(relative(fraction)),
        }
    }
}

/// A row of a skeleton block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkeletonRow {
    /// Bar width.
    pub width: BarWidth,
    /// Bar height.
    pub height: f32,
}

/// `count` rows of `height` whose widths follow the deterministic ladder
/// (0.62, 0.48, 0.71, 0.55, 0.39, 0.66, repeating), so consecutive rows read as varied text.
pub fn skeleton_rows(count: usize, height: f32) -> Vec<SkeletonRow> {
    (0..count)
        .map(|row| SkeletonRow {
            width: BarWidth::Fraction(ladder_width(row)),
            height,
        })
        .collect()
}

/// An accessible block of skeleton rows: one progress-indicator node labeled with `label`
/// (for example "Loading messages"); the bars inside are hidden. The pulse lives in its own
/// cached sub-view keyed by `key`, so only the bars repaint on each tick.
#[derive(IntoElement)]
pub struct SkeletonBlock {
    key: ElementId,
    label: SharedString,
    rows: Vec<SkeletonRow>,
    gap: f32,
    coarse: bool,
}

/// A skeleton block (see [`SkeletonBlock`]).
pub fn skeleton_block(
    key: impl Into<ElementId>,
    label: impl Into<SharedString>,
    rows: Vec<SkeletonRow>,
) -> SkeletonBlock {
    SkeletonBlock {
        key: key.into(),
        label: label.into(),
        rows,
        gap: 8.0,
        coarse: false,
    }
}

impl SkeletonBlock {
    /// Vertical gap between rows (default 8 px).
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    /// Runs the pulse at stride 2 (about 15 Hz) for coarse chrome.
    pub fn coarse(mut self, coarse: bool) -> Self {
        self.coarse = coarse;
        self
    }

    /// The block's height: row heights plus gaps.
    pub fn height(&self) -> f32 {
        let heights: f32 = self.rows.iter().map(|row| row.height).sum();
        heights + self.gap * self.rows.len().saturating_sub(1) as f32
    }
}

impl RenderOnce for SkeletonBlock {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let height = self.height();
        let kind = IndicatorKind::Skeleton {
            rows: self.rows,
            label: self.label,
            gap: self.gap,
        };
        let key = self.key;
        let view = window.use_keyed_state(key.clone(), cx, {
            let kind = kind.clone();
            let coarse = self.coarse;
            move |_, _| IndicatorView::new(key, kind, coarse)
        });
        view.update(cx, |view, cx| view.set(kind, self.coarse, cx));
        let style = StyleRefinement::default()
            .w_full()
            .h(px(height))
            .flex_none();
        view.cached(style).into_any_element()
    }
}

/// Builds the bars of a block for the shared-clock `phase` (used by the cached view).
pub(crate) fn block_element(
    id: ElementId,
    label: SharedString,
    rows: &[SkeletonRow],
    gap: f32,
    phase: f32,
) -> impl IntoElement {
    let mut block = div()
        .id(id)
        .role(Role::ProgressIndicator)
        .aria_label(label)
        .flex()
        .flex_col()
        .w_full()
        .gap(px(gap));
    for (index, row) in rows.iter().enumerate() {
        block = block.child(skeleton_bar(row.width, row.height, index).phase(phase));
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_the_width_ladder_and_repeat() {
        let rows = skeleton_rows(8, 10.0);
        let widths: Vec<f32> = rows
            .iter()
            .map(|row| match row.width {
                BarWidth::Fraction(f) => f,
                BarWidth::Px(_) => unreachable!(),
            })
            .collect();
        assert_eq!(widths, [0.62, 0.48, 0.71, 0.55, 0.39, 0.66, 0.62, 0.48]);
    }

    #[test]
    fn block_height_is_rows_plus_gaps() {
        let block = skeleton_block("b", "Loading", skeleton_rows(3, 10.0)).gap(6.0);
        assert_eq!(block.height(), 42.0);
        let empty = skeleton_block("b", "Loading", Vec::new());
        assert_eq!(empty.height(), 0.0);
    }
}
