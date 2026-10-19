//! The per-frame document-ordered text registry (TXT-008 R1-R4) and the inspection logs of
//! TXT-008/TXT-009 (painted text, painted washes).

use crate::display_map::OriginalText;
use crate::geometry::{IndexHit, RichLayout};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{Bounds, Pixels, Point, px};

/// Stable key of a painted run: owner identity plus a sub-index (a code view registers one run
/// per line).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunKey {
    /// Hash of the owning element identity.
    pub owner: u64,
    /// Sub-index within the owner.
    pub sub: u32,
}

impl RunKey {
    /// A key from any hashable owner identity and a sub-index.
    pub fn new(owner: impl Hash, sub: u32) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        owner.hash(&mut hasher);
        Self {
            owner: hasher.finish(),
            sub,
        }
    }
}

/// Identity of a painted surface (a document in a modal versus the main document). Run keys are
/// only unique within one surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct SurfaceId(pub u64);

/// Shaped geometry of one registered run.
pub trait RunGeometry {
    /// Bounds of the whole run in window coordinates.
    fn bounds(&self) -> Bounds<Pixels>;
    /// Byte index at a window position, exact or nearest.
    fn index_for_position(&self, position: Point<Pixels>) -> IndexHit;
    /// Wrapped rectangles of a byte range (one per visual row, one line high, no padding).
    fn range_rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>>;
}

impl RunGeometry for RichLayout {
    fn bounds(&self) -> Bounds<Pixels> {
        RichLayout::bounds(self)
    }

    fn index_for_position(&self, position: Point<Pixels>) -> IndexHit {
        RichLayout::index_for_position(self, position)
    }

    fn range_rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        RichLayout::range_rects(self, range, px(0.), px(0.))
    }
}

/// One selectable run registered while painting.
#[derive(Clone)]
pub struct RegisteredRun {
    /// Stable key.
    pub key: RunKey,
    /// The surface that painted it.
    pub surface: SurfaceId,
    /// The text it painted (displayed text) together with its source mapping.
    pub text: Arc<OriginalText>,
    /// Shaped layout.
    pub geometry: Rc<dyn RunGeometry>,
    /// Group identity: the first run of a maximal sequence of adjacent text children of one host.
    pub group: Option<RunKey>,
}

/// A rectangle that decides whether a press may start a selection (R2, R3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartRegion {
    /// Painted box.
    pub bounds: Bounds<Pixels>,
    /// `false` for editable hosts and chrome, `true` for an explicit `text` island.
    pub selectable: bool,
}

/// Entry of the painted-text log (R12 of TXT-009, R1/R11 of TXT-008).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaintedText {
    /// Element identity hash and sub-index.
    pub key: RunKey,
    /// The full painted string.
    pub text: String,
    /// Whether it was painted through the chrome path (logged, never registered).
    pub chrome: bool,
}

/// Entry of the painted-highlight log (TXT-009 R12).
#[derive(Clone, Debug, PartialEq)]
pub struct PaintedHighlight {
    /// Element identity hash and sub-index of the run.
    pub key: RunKey,
    /// The run's full text.
    pub text: String,
    /// UTF-16 start of the wash within the text.
    pub start: usize,
    /// UTF-16 end of the wash within the text.
    pub end: usize,
    /// Whether this is the active match.
    pub active: bool,
    /// One rectangle per painted row: (x, y, width, height).
    pub rects: Vec<[f32; 4]>,
}

/// Outcome of resolving a press position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressTarget {
    /// The last painted start region containing the pointer is not selectable.
    Blocked,
    /// No eligible run (outside every run's vertical band, or no runs).
    NoRun,
    /// A run and the byte index in it (exact hit, otherwise nearest).
    Run {
        /// Position in registry order.
        run: usize,
        /// Byte index in the run's displayed text.
        index: usize,
    },
}

/// The document-ordered registry of one frame.
#[derive(Default)]
pub struct Registry {
    runs: Vec<RegisteredRun>,
    regions: Vec<StartRegion>,
    painted_text: Vec<PaintedText>,
    painted_highlights: Vec<PaintedHighlight>,
}

fn distance(value: Pixels, low: Pixels, high: Pixels) -> f32 {
    if value < low {
        f32::from(low - value)
    } else if value > high {
        f32::from(value - high)
    } else {
        0.0
    }
}

impl Registry {
    /// Clears runs, regions and both logs: the frame reset.
    pub fn reset(&mut self) {
        self.runs.clear();
        self.regions.clear();
        self.painted_text.clear();
        self.painted_highlights.clear();
    }

    /// Registers a run in paint order. A run that paints twice in one frame keeps its first
    /// position and takes the latest geometry.
    pub fn register(&mut self, run: RegisteredRun) {
        if let Some(existing) = self
            .runs
            .iter_mut()
            .find(|existing| existing.key == run.key && existing.surface == run.surface)
        {
            *existing = run;
        } else {
            self.runs.push(run);
        }
    }

    /// Registers a start region.
    pub fn register_region(&mut self, region: StartRegion) {
        self.regions.push(region);
    }

    /// Records a painted string.
    pub fn log_text(&mut self, entry: PaintedText) {
        self.painted_text.push(entry);
    }

    /// Records a painted wash.
    pub fn log_highlight(&mut self, entry: PaintedHighlight) {
        self.painted_highlights.push(entry);
    }

    /// Registered runs in paint (document) order.
    pub fn runs(&self) -> &[RegisteredRun] {
        &self.runs
    }

    /// Start regions in paint order.
    pub fn regions(&self) -> &[StartRegion] {
        &self.regions
    }

    /// Strings painted this frame, in paint order.
    pub fn painted_text(&self) -> &[PaintedText] {
        &self.painted_text
    }

    /// Washes painted this frame, in paint order.
    pub fn painted_highlights(&self) -> &[PaintedHighlight] {
        &self.painted_highlights
    }

    /// Position of the run with `key` on `surface`.
    pub fn run_index(&self, surface: SurfaceId, key: RunKey) -> Option<usize> {
        self.runs
            .iter()
            .position(|run| run.surface == surface && run.key == key)
    }

    /// Where a press at `point` may start a selection (R3).
    pub fn press_target(&self, point: Point<Pixels>) -> PressTarget {
        if let Some(region) = self
            .regions
            .iter()
            .rev()
            .find(|region| region.bounds.contains(&point))
            && !region.selectable
        {
            return PressTarget::Blocked;
        }
        let Some(run) = self.nearest_run(point, None) else {
            return PressTarget::NoRun;
        };
        let bounds = self.runs[run].geometry.bounds();
        if point.y < bounds.top() || point.y > bounds.bottom() {
            return PressTarget::NoRun;
        }
        PressTarget::Run {
            run,
            index: self.runs[run].geometry.index_for_position(point).index(),
        }
    }

    /// The drag head (R4): the unbounded nearest rule, restricted to `surface` when given.
    pub fn drag_head(
        &self,
        point: Point<Pixels>,
        surface: Option<SurfaceId>,
    ) -> Option<(usize, usize)> {
        let run = self.nearest_run(point, surface)?;
        Some((
            run,
            self.runs[run].geometry.index_for_position(point).index(),
        ))
    }

    /// The "on-line" rule: the last run whose bounds contain the point; otherwise the nearest run
    /// by vertical distance, then horizontal distance (lexicographic).
    fn nearest_run(&self, point: Point<Pixels>, surface: Option<SurfaceId>) -> Option<usize> {
        let eligible = |run: &RegisteredRun| surface.is_none_or(|surface| run.surface == surface);
        if let Some(contained) = self
            .runs
            .iter()
            .enumerate()
            .rev()
            .find(|(_, run)| eligible(run) && run.geometry.bounds().contains(&point))
        {
            return Some(contained.0);
        }
        let mut best: Option<(usize, (f32, f32))> = None;
        for (ix, run) in self
            .runs
            .iter()
            .enumerate()
            .filter(|(_, run)| eligible(run))
        {
            let bounds = run.geometry.bounds();
            let key = (
                distance(point.y, bounds.top(), bounds.bottom()),
                distance(point.x, bounds.left(), bounds.right()),
            );
            if best.is_none_or(|(_, best_key)| key <= best_key) {
                best = Some((ix, key));
            }
        }
        best.map(|(ix, _)| ix)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use vespertine::{point, size};

    /// A model run: fixed 10 px glyphs, one row of `top..top+24`, starting at x.
    pub struct ModelRun {
        pub bounds: Bounds<Pixels>,
        pub len: usize,
    }

    impl ModelRun {
        pub fn new(x: f32, top: f32, chars: usize) -> Rc<Self> {
            Rc::new(Self {
                bounds: Bounds {
                    origin: point(px(x), px(top)),
                    size: size(px(chars as f32 * 10.0), px(24.0)),
                },
                len: chars,
            })
        }
    }

    impl RunGeometry for ModelRun {
        fn bounds(&self) -> Bounds<Pixels> {
            self.bounds
        }

        fn index_for_position(&self, position: Point<Pixels>) -> IndexHit {
            let x = f32::from(position.x - self.bounds.origin.x);
            if x < 0.0 {
                IndexHit::Nearest(0)
            } else if x >= self.len as f32 * 10.0 {
                IndexHit::Nearest(self.len)
            } else {
                IndexHit::Exact((x / 10.0) as usize)
            }
        }

        fn range_rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
            if range.start >= range.end {
                return vec![];
            }
            vec![Bounds {
                origin: point(
                    self.bounds.origin.x + px(range.start as f32 * 10.0),
                    self.bounds.origin.y,
                ),
                size: size(px((range.end - range.start) as f32 * 10.0), px(24.0)),
            }]
        }
    }

    pub fn run(
        key: u32,
        surface: u64,
        text: &str,
        top: f32,
        group: Option<RunKey>,
    ) -> RegisteredRun {
        RegisteredRun {
            key: RunKey::new("test", key),
            surface: SurfaceId(surface),
            text: Arc::new(OriginalText::plain(text)),
            geometry: ModelRun::new(0.0, top, text.chars().count()),
            group,
        }
    }
}
