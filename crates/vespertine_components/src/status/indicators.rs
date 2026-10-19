//! Indeterminate indicators (CMP-023 R6, R7, R13, R14): wave row, matrix spinner, mini glyph
//! and pulse glow. They animate only opacity and inner size inside fixed slots, from the shared
//! pulse clock, each in its own cached sub-view so that surrounding content is reused.

use vespertine::{
    AnyElement, App, Context, ElementId, Hsla, IntoElement, ParentElement, Render, RenderOnce,
    SharedString, StyleRefinement, Styled, Window, div, px,
};

use super::motion::{
    SLOW_PERIOD, WAVE_REST_OPACITY, WAVE_REST_SIZE, glow_opacity, matrix_cell_opacity,
    mini_cell_opacity, wave_cell,
};
use super::skeleton::{SkeletonRow, block_element};
use super::theme::{Activity, StatusColors, activity, pulse};

/// What an indicator view draws.
#[derive(Clone, Debug, PartialEq)]
pub enum IndicatorKind {
    /// A row of `cells` cells of size `cell`.
    WaveRow {
        /// Cell count.
        cells: usize,
        /// Cell size.
        cell: f32,
    },
    /// The 3 x 3 matrix spinner.
    Matrix {
        /// Cell size.
        cell: f32,
        /// One tint per row.
        tints: [Hsla; 3],
    },
    /// The 2 x 3 mini glyph.
    Mini {
        /// Cell size.
        cell: f32,
        /// One tint per row.
        tints: [Hsla; 3],
    },
    /// A rounded fill breathing between a floor and a peak opacity.
    Glow {
        /// Corner radius.
        radius: f32,
        /// Fill.
        tint: Hsla,
        /// Lowest opacity.
        floor: f32,
        /// Highest opacity.
        peak: f32,
    },
    /// A skeleton block.
    Skeleton {
        /// Rows.
        rows: Vec<SkeletonRow>,
        /// Accessible label.
        label: SharedString,
        /// Gap between rows.
        gap: f32,
    },
}

/// The retained view of one indicator.
pub struct IndicatorView {
    id: ElementId,
    kind: IndicatorKind,
    coarse: bool,
}

impl IndicatorView {
    pub(crate) fn new(id: ElementId, kind: IndicatorKind, coarse: bool) -> Self {
        Self { id, kind, coarse }
    }

    /// Replaces the configuration; notifies only on change.
    pub(crate) fn set(&mut self, kind: IndicatorKind, coarse: bool, cx: &mut Context<Self>) {
        if self.kind != kind || self.coarse != coarse {
            self.kind = kind;
            self.coarse = coarse;
            cx.notify();
        }
    }

    /// The opacity of every cell of a wave row at the current clock: `(opacity, inner size)`.
    fn wave_values(
        window: &mut Window,
        cx: &mut App,
        coarse: bool,
        cells: usize,
    ) -> Vec<(f32, f32)> {
        let clock = pulse(window, cx, SLOW_PERIOD, coarse);
        (0..cells)
            .map(|index| {
                if clock.reduced {
                    (WAVE_REST_OPACITY, WAVE_REST_SIZE)
                } else {
                    wave_cell(clock.phase, index)
                }
            })
            .collect()
    }
}

/// Slot size of an indicator, when it has a fixed intrinsic slot.
pub fn slot_size(kind: &IndicatorKind) -> Option<(f32, f32)> {
    match kind {
        IndicatorKind::WaveRow { cells, cell } => {
            let n = *cells as f32;
            Some((n * cell + (n - 1.0).max(0.0) * cell / 2.0, *cell))
        }
        IndicatorKind::Matrix { cell, .. } => Some((4.0 * cell, 4.0 * cell)),
        IndicatorKind::Mini { cell, .. } => Some((2.5 * cell, 4.0 * cell)),
        IndicatorKind::Glow { .. } | IndicatorKind::Skeleton { .. } => None,
    }
}

impl Render for IndicatorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = StatusColors::active(cx);
        match self.kind.clone() {
            IndicatorKind::WaveRow { cells, cell } => {
                let values = Self::wave_values(window, cx, self.coarse, cells);
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(cell / 2.0))
                    .h(px(cell));
                for (opacity, inner) in values {
                    row = row.child(
                        div()
                            .w(px(cell))
                            .h(px(cell))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .w(px(cell * inner))
                                    .h(px(cell * inner))
                                    .rounded(px(cell / 4.0))
                                    .bg(colors.text)
                                    .opacity(opacity),
                            ),
                    );
                }
                row.into_any_element()
            }
            IndicatorKind::Matrix { cell, tints } => {
                let mode = activity(window, cx);
                let mut grid = div().flex().flex_col().gap(px(cell / 2.0));
                for (row, tint) in tints.iter().enumerate() {
                    let mut line = div().flex().flex_row().gap(px(cell / 2.0));
                    for col in 0..3 {
                        let opacity = match mode {
                            Activity::Chase(phase) => matrix_cell_opacity(phase, row, col),
                            Activity::Uniform(opacity) => opacity,
                        };
                        line = line.child(
                            div()
                                .w(px(cell))
                                .h(px(cell))
                                .flex_none()
                                .rounded(px(cell / 2.0))
                                .bg(*tint)
                                .opacity(opacity),
                        );
                    }
                    grid = grid.child(line);
                }
                grid.into_any_element()
            }
            IndicatorKind::Mini { cell, tints } => {
                let mode = activity(window, cx);
                let mut grid = div()
                    .flex()
                    .flex_col()
                    .gap(px(cell / 2.0))
                    .w(px(2.5 * cell))
                    .h(px(4.0 * cell));
                for (row, tint) in tints.iter().enumerate() {
                    let mut line = div().flex().flex_row().gap(px(cell / 2.0));
                    for col in 0..2 {
                        let opacity = match mode {
                            Activity::Chase(phase) => mini_cell_opacity(phase, row, col),
                            Activity::Uniform(opacity) => opacity,
                        };
                        line = line.child(
                            div()
                                .w(px(cell))
                                .h(px(cell))
                                .flex_none()
                                .rounded(px(cell / 2.0))
                                .bg(*tint)
                                .opacity(opacity),
                        );
                    }
                    grid = grid.child(line);
                }
                grid.into_any_element()
            }
            IndicatorKind::Glow {
                radius,
                tint,
                floor,
                peak,
            } => {
                let clock = pulse(window, cx, SLOW_PERIOD, self.coarse);
                div()
                    .absolute()
                    .size_full()
                    .rounded(px(radius))
                    .bg(tint)
                    .opacity(glow_opacity(clock.phase, floor, peak))
                    .into_any_element()
            }
            IndicatorKind::Skeleton { rows, label, gap } => {
                let clock = pulse(window, cx, SLOW_PERIOD, self.coarse);
                block_element(self.id.clone(), label, &rows, gap, clock.phase).into_any_element()
            }
        }
    }
}

fn indicator_element(
    key: ElementId,
    kind: IndicatorKind,
    coarse: bool,
    style: StyleRefinement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let view = window.use_keyed_state(key.clone(), cx, {
        let kind = kind.clone();
        move |_, _| IndicatorView::new(key, kind, coarse)
    });
    view.update(cx, |view, cx| view.set(kind, coarse, cx));
    view.cached(style).into_any_element()
}

fn slot_style(kind: &IndicatorKind) -> StyleRefinement {
    match slot_size(kind) {
        Some((w, h)) => StyleRefinement::default().w(px(w)).h(px(h)).flex_none(),
        None => StyleRefinement::default().size_full(),
    }
}

/// The wave row: `cells` rounded squares (default 5) in a slot of `cell` height and
/// `n x cell + (n - 1) x cell / 2` width.
#[derive(IntoElement)]
pub struct WaveLoader {
    key: ElementId,
    cells: usize,
    cell: f32,
    coarse: bool,
}

/// A wave row loader of cell size `cell`, keyed by `key`.
pub fn wave_loader(key: impl Into<ElementId>, cell: f32) -> WaveLoader {
    WaveLoader {
        key: key.into(),
        cells: 5,
        cell,
        coarse: false,
    }
}

impl WaveLoader {
    /// Sets the cell count.
    pub fn cells(mut self, cells: usize) -> Self {
        self.cells = cells.max(1);
        self
    }

    /// Runs the pulse at stride 2 (coarse chrome).
    pub fn coarse(mut self, coarse: bool) -> Self {
        self.coarse = coarse;
        self
    }
}

impl RenderOnce for WaveLoader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let kind = IndicatorKind::WaveRow {
            cells: self.cells,
            cell: self.cell,
        };
        let style = slot_style(&kind);
        indicator_element(self.key, kind, self.coarse, style, window, cx)
    }
}

/// The 3 x 3 matrix spinner (750 ms period): each row has its own tint (default: the accent).
#[derive(IntoElement)]
pub struct MatrixSpinner {
    key: ElementId,
    cell: f32,
    tints: Option<[Hsla; 3]>,
}

/// A matrix spinner of cell size `cell`, keyed by `key`.
pub fn matrix_spinner(key: impl Into<ElementId>, cell: f32) -> MatrixSpinner {
    MatrixSpinner {
        key: key.into(),
        cell,
        tints: None,
    }
}

impl MatrixSpinner {
    /// Sets one tint per row.
    pub fn tints(mut self, tints: [Hsla; 3]) -> Self {
        self.tints = Some(tints);
        self
    }
}

impl RenderOnce for MatrixSpinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let accent = StatusColors::active(cx).accent;
        let kind = IndicatorKind::Matrix {
            cell: self.cell,
            tints: self.tints.unwrap_or([accent; 3]),
        };
        let style = slot_style(&kind);
        indicator_element(self.key, kind, false, style, window, cx)
    }
}

/// The 2 x 3 mini glyph: brightness chases around the perimeter (cell phase `ring / 6`).
#[derive(IntoElement)]
pub struct MiniGlyph {
    key: ElementId,
    cell: f32,
    tints: Option<[Hsla; 3]>,
}

/// A mini glyph spinner of cell size `cell` (typically 2.5 px), keyed by `key`.
pub fn mini_glyph_spinner(key: impl Into<ElementId>, cell: f32) -> MiniGlyph {
    MiniGlyph {
        key: key.into(),
        cell,
        tints: None,
    }
}

impl MiniGlyph {
    /// Sets one tint per row.
    pub fn tints(mut self, tints: [Hsla; 3]) -> Self {
        self.tints = Some(tints);
        self
    }

    /// One tint for every row (for surfaces where an accent would pull focus).
    pub fn tint(mut self, tint: Hsla) -> Self {
        self.tints = Some([tint; 3]);
        self
    }
}

impl RenderOnce for MiniGlyph {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let accent = StatusColors::active(cx).accent;
        let kind = IndicatorKind::Mini {
            cell: self.cell,
            tints: self.tints.unwrap_or([accent; 3]),
        };
        let style = slot_style(&kind);
        indicator_element(self.key, kind, false, style, window, cx)
    }
}

/// A rounded fill that fills a positioned box and breathes between `floor` and `peak` opacity.
#[derive(IntoElement)]
pub struct PulseGlow {
    key: ElementId,
    radius: f32,
    tint: Hsla,
    floor: f32,
    peak: f32,
    coarse: bool,
}

/// A pulse glow (`opacity = floor + (peak - floor) x wave(clock)`).
pub fn pulse_glow(
    key: impl Into<ElementId>,
    radius: f32,
    tint: Hsla,
    floor: f32,
    peak: f32,
) -> PulseGlow {
    PulseGlow {
        key: key.into(),
        radius,
        tint,
        floor,
        peak,
        coarse: false,
    }
}

impl PulseGlow {
    /// Runs the pulse at stride 2 (coarse chrome).
    pub fn coarse(mut self, coarse: bool) -> Self {
        self.coarse = coarse;
        self
    }
}

impl RenderOnce for PulseGlow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let kind = IndicatorKind::Glow {
            radius: self.radius,
            tint: self.tint,
            floor: self.floor,
            peak: self.peak,
        };
        let style = slot_style(&kind);
        indicator_element(self.key, kind, self.coarse, style, window, cx)
    }
}

/// A count rendered through a cap: exact up to `cap`, then `"{cap}+"` (R7). The value itself is
/// never modified.
pub fn capped_label(count: u32, cap: u32) -> String {
    if count > cap {
        format!("{cap}+")
    } else {
        count.to_string()
    }
}

/// The widest form of a capped label (`"{cap}+"`): the width a count slot reserves, so changing
/// the count never changes the slot width.
pub fn widest_capped_label(cap: u32) -> String {
    format!("{cap}+")
}

/// The slot width of a count label: the width of its widest capped form, measured by `measure`.
pub fn count_slot_width(cap: u32, measure: &dyn Fn(&str) -> f32) -> f32 {
    measure(&widest_capped_label(cap))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a9_count_label_caps_and_slot_width_is_the_widest_form() {
        assert_eq!(capped_label(0, 99), "0");
        assert_eq!(capped_label(99, 99), "99");
        assert_eq!(capped_label(100, 99), "99+");
        assert_eq!(capped_label(120, 99), "99+");
        assert_eq!(capped_label(120, 9), "9+");
        let measure = |text: &str| text.chars().count() as f32 * 7.0;
        // The slot is as wide as "99+" whatever the current count is.
        let slot = count_slot_width(99, &measure);
        assert_eq!(slot, measure("99+"));
        assert!(slot >= measure(&capped_label(120, 99)));
        assert!(slot >= measure(&capped_label(7, 99)));
    }

    #[test]
    fn slots_are_fixed_by_the_cell_size() {
        let wave = IndicatorKind::WaveRow {
            cells: 5,
            cell: 4.0,
        };
        assert_eq!(slot_size(&wave), Some((5.0 * 4.0 + 4.0 * 2.0, 4.0)));
        let matrix = IndicatorKind::Matrix {
            cell: 3.0,
            tints: [Hsla::default(); 3],
        };
        assert_eq!(slot_size(&matrix), Some((12.0, 12.0)));
        let mini = IndicatorKind::Mini {
            cell: 2.5,
            tints: [Hsla::default(); 3],
        };
        assert_eq!(slot_size(&mini), Some((6.25, 10.0)));
        let glow = IndicatorKind::Glow {
            radius: 4.0,
            tint: Hsla::default(),
            floor: 0.2,
            peak: 0.8,
        };
        assert_eq!(slot_size(&glow), None);
    }
}
