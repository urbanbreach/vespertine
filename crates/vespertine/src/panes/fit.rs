//! Pane descriptors, retained per-pane state and the pure fitting function
//! (LAY-004 R1, R3).

use crate::{Pixels, SharedString, Size, px};

/// Leading sidebar preset: minimum width.
pub const LEADING_MIN_WIDTH: Pixels = px(224.);
/// Leading sidebar preset: maximum width.
pub const LEADING_MAX_WIDTH: Pixels = px(400.);
/// Leading sidebar preset: default width.
pub const LEADING_DEFAULT_WIDTH: Pixels = px(256.);
/// Trailing surface preset: minimum width.
pub const TRAILING_MIN_WIDTH: Pixels = px(360.);
/// Trailing surface preset: default width.
pub const TRAILING_DEFAULT_WIDTH: Pixels = px(520.);
/// Default minimum width of the main content region.
pub const MAIN_MIN_WIDTH: Pixels = px(300.);
/// Bottom surface preset: minimum height.
pub const BOTTOM_MIN_HEIGHT: Pixels = px(160.);
/// Bottom surface preset: default height.
pub const BOTTOM_DEFAULT_HEIGHT: Pixels = px(280.);
/// Bottom surface height is limited to this fraction of the viewport height.
pub const BOTTOM_MAX_VIEWPORT_FRACTION: f32 = 0.55;
/// Absolute maximum bottom surface height (also bounds restored values).
pub const BOTTOM_ABSOLUTE_MAX_HEIGHT: Pixels = px(2000.);

/// Stable identity of a pane.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub SharedString);

impl From<&'static str> for PaneId {
    fn from(value: &'static str) -> Self {
        Self(SharedString::new_static(value))
    }
}

impl From<String> for PaneId {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

impl From<SharedString> for PaneId {
    fn from(value: SharedString) -> Self {
        Self(value)
    }
}

/// The edge of the group a pane is attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaneSide {
    /// A sidebar at the leading (left) edge; its size is a width.
    Leading,
    /// A surface at the trailing (right) edge; its size is a width.
    Trailing,
    /// The bottom-docked surface; its size is a height.
    Bottom,
}

impl PaneSide {
    /// Whether the pane shares the horizontal axis with the main region.
    pub fn is_horizontal(self) -> bool {
        !matches!(self, PaneSide::Bottom)
    }
}

/// Static configuration of one pane.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneDescriptor {
    /// The pane's identity.
    pub id: PaneId,
    /// The edge the pane is attached to.
    pub side: PaneSide,
    /// Initial size (width, or height for the bottom pane).
    pub preferred_size: Pixels,
    /// Minimum size.
    pub min_size: Pixels,
    /// Maximum size.
    pub max_size: Pixels,
    /// Size a seam double-click resets to (re-clamped to current limits).
    pub default_size: Pixels,
    /// Fitting priority: panes with the lowest priority are hidden first.
    pub priority: i32,
    /// A takeover pane is hidden last and may use the main region's space
    /// down to a main minimum of zero while it is the only pane left.
    pub takeover: bool,
    /// Whether the pane starts collapsed.
    pub collapsed: bool,
}

impl PaneDescriptor {
    /// Creates a descriptor with the preset sizes for `side`.
    pub fn new(id: impl Into<PaneId>, side: PaneSide) -> Self {
        let (min, max, default) = match side {
            PaneSide::Leading => (LEADING_MIN_WIDTH, LEADING_MAX_WIDTH, LEADING_DEFAULT_WIDTH),
            PaneSide::Trailing => (TRAILING_MIN_WIDTH, Pixels::MAX, TRAILING_DEFAULT_WIDTH),
            PaneSide::Bottom => (
                BOTTOM_MIN_HEIGHT,
                BOTTOM_ABSOLUTE_MAX_HEIGHT,
                BOTTOM_DEFAULT_HEIGHT,
            ),
        };
        Self {
            id: id.into(),
            side,
            preferred_size: default,
            min_size: min,
            max_size: max,
            default_size: default,
            priority: 0,
            takeover: false,
            collapsed: false,
        }
    }

    /// A leading sidebar with the preset sizes.
    pub fn leading(id: impl Into<PaneId>) -> Self {
        Self::new(id, PaneSide::Leading)
    }

    /// A trailing surface with the preset sizes.
    pub fn trailing(id: impl Into<PaneId>) -> Self {
        Self::new(id, PaneSide::Trailing)
    }

    /// The bottom-docked surface with the preset sizes.
    pub fn bottom(id: impl Into<PaneId>) -> Self {
        Self::new(id, PaneSide::Bottom)
    }

    /// Sets the initial size.
    pub fn preferred_size(mut self, size: Pixels) -> Self {
        self.preferred_size = size;
        self
    }

    /// Sets the minimum size.
    pub fn min_size(mut self, size: Pixels) -> Self {
        self.min_size = size;
        self
    }

    /// Sets the maximum size.
    pub fn max_size(mut self, size: Pixels) -> Self {
        self.max_size = size;
        self
    }

    /// Sets the size a seam reset returns to.
    pub fn default_size(mut self, size: Pixels) -> Self {
        self.default_size = size;
        self
    }

    /// Sets the fitting priority (lowest is hidden first).
    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Marks the pane as taking over the main region's space.
    pub fn takeover(mut self, takeover: bool) -> Self {
        self.takeover = takeover;
        self
    }

    /// Sets whether the pane starts collapsed.
    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    /// Clamps `size` to this pane's own minimum and maximum.
    pub fn clamp_size(&self, size: Pixels) -> Pixels {
        let max = self.max_size.max(self.min_size);
        size.max(self.min_size).min(max)
    }
}

/// Retained, caller-observable state of one pane (LAY-004 R1, R7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneState {
    /// The retained size preference. It survives hiding, collapsing and
    /// window resizes; the size actually used is derived by [`fit`].
    pub size: Pixels,
    /// Whether the pane's open flag is off.
    pub collapsed: bool,
    /// Serial number of the pane's most recent opening (the initial opening
    /// counts); lower means opened earlier.
    pub opened_order: u64,
    /// Serial number of the most recent direct size change (drag, reset or
    /// [`PaneGroupState::set_size`](super::PaneGroupState::set_size)); zero
    /// when never resized. When panes compete for space the pane adjusted
    /// most recently keeps its width longest.
    pub adjustment: u64,
}

/// A pane's configuration together with its retained state.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneEntry {
    /// Static configuration.
    pub descriptor: PaneDescriptor,
    /// Retained state.
    pub state: PaneState,
}

impl PaneEntry {
    /// Whether the open flag is on.
    pub fn is_open(&self) -> bool {
        !self.state.collapsed
    }
}

/// Size limits of a pane along its axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneLimits {
    /// Smallest size the pane may have while visible.
    pub min: Pixels,
    /// Largest size the pane may currently have.
    pub max: Pixels,
}

impl PaneLimits {
    /// Clamps `size` into the limits.
    pub fn clamp(&self, size: Pixels) -> Pixels {
        size.max(self.min).min(self.max.max(self.min))
    }
}

/// The fitted result for one pane.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneFitResult {
    /// The pane.
    pub id: PaneId,
    /// The pane's side.
    pub side: PaneSide,
    /// The pane's open flag (not affected by fitting).
    pub open: bool,
    /// Whether the pane is shown: open and not hidden by fitting.
    pub visible: bool,
    /// The size the pane occupies at rest: zero unless visible.
    pub size: Pixels,
    /// The size limits that currently apply to the pane.
    pub limits: PaneLimits,
}

/// The result of [`fit`].
#[derive(Clone, Debug, PartialEq)]
pub struct PaneFit {
    /// One result per input pane, in input order.
    pub panes: Vec<PaneFitResult>,
    /// Width left to the main region at rest.
    pub main_width: Pixels,
    /// The main region's minimum width that applied (zero for a lone takeover).
    pub main_min: Pixels,
    /// Height of the visible bottom pane at rest, zero if none.
    pub bottom_height: Pixels,
}

impl PaneFit {
    /// The result for `id`.
    pub fn pane(&self, id: &PaneId) -> Option<&PaneFitResult> {
        self.panes.iter().find(|pane| &pane.id == id)
    }
}

/// Maps a viewport size and the pane entries to visible panes and size
/// limits (LAY-004 R3).
///
/// Open horizontal panes are hidden one at a time while `main_min` plus the
/// minimums of all open horizontal panes exceeds the viewport width: the pane
/// with the lowest priority goes first, then the earliest opened; takeover
/// panes go after all others, and a lone takeover pane may shrink the main
/// minimum to zero. A pane's width limit is
/// `min(max, viewport - main_min - minimums of the other visible panes)`.
/// When the preferred widths together exceed the space, the pane adjusted
/// least recently (trailing before leading on ties) gives up width first,
/// never below its minimum. The bottom pane's height is limited to its
/// maximum, the absolute maximum and 55 percent of the viewport height, but
/// never below its minimum and never above the viewport height.
pub fn fit(viewport: Size<Pixels>, main_min: Pixels, panes: &[PaneEntry]) -> PaneFit {
    let viewport_width = viewport.width.0.max(0.0);
    let viewport_height = viewport.height.0.max(0.0);
    let main_min = main_min.0.max(0.0);

    let mut visible: Vec<usize> = panes
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.descriptor.side.is_horizontal() && entry.is_open())
        .map(|(index, _)| index)
        .collect();

    let min_of = |entry: &PaneEntry| entry.descriptor.min_size.0.max(0.0);
    let effective_main_min = |visible: &[usize]| match visible {
        [only] if panes[*only].descriptor.takeover => 0.0,
        _ => main_min,
    };

    loop {
        if visible.is_empty() {
            break;
        }
        let needed =
            effective_main_min(&visible) + visible.iter().map(|&i| min_of(&panes[i])).sum::<f32>();
        if needed <= viewport_width {
            break;
        }
        let hide = visible
            .iter()
            .copied()
            .min_by_key(|&i| {
                let entry = &panes[i];
                (
                    entry.descriptor.takeover,
                    entry.descriptor.priority,
                    entry.state.opened_order,
                    i,
                )
            })
            .expect("visible is non-empty");
        visible.retain(|&i| i != hide);
    }

    let main_min_used = effective_main_min(&visible);
    let available = (viewport_width - main_min_used).max(0.0);
    let min_sum: f32 = visible.iter().map(|&i| min_of(&panes[i])).sum();

    let mut widths: Vec<(usize, f32)> = visible
        .iter()
        .map(|&i| (i, panes[i].descriptor.clamp_size(panes[i].state.size).0))
        .collect();
    let mut excess = widths.iter().map(|(_, w)| *w).sum::<f32>() - available;
    if excess > 0.0 {
        let mut order: Vec<usize> = (0..widths.len()).collect();
        order.sort_by_key(|&slot| {
            let index = widths[slot].0;
            let entry = &panes[index];
            (
                entry.state.adjustment,
                entry.descriptor.side != PaneSide::Trailing,
                std::cmp::Reverse(index),
            )
        });
        for slot in order {
            if excess <= 0.0 {
                break;
            }
            let (index, width) = widths[slot];
            let take = excess.min((width - min_of(&panes[index])).max(0.0));
            widths[slot].1 = width - take;
            excess -= take;
        }
    }

    let mut results: Vec<PaneFitResult> = panes
        .iter()
        .map(|entry| PaneFitResult {
            id: entry.descriptor.id.clone(),
            side: entry.descriptor.side,
            open: entry.is_open(),
            visible: false,
            size: Pixels::ZERO,
            limits: PaneLimits {
                min: entry.descriptor.min_size,
                max: entry.descriptor.max_size.max(entry.descriptor.min_size),
            },
        })
        .collect();

    let mut used = 0.0;
    for &(index, width) in &widths {
        let entry = &panes[index];
        let others_min = min_sum - min_of(entry);
        let max = (entry.descriptor.max_size.0)
            .min(viewport_width - main_min_used - others_min)
            .max(min_of(entry));
        results[index].visible = true;
        results[index].size = Pixels(width);
        results[index].limits = PaneLimits {
            min: entry.descriptor.min_size,
            max: Pixels(max),
        };
        used += width;
    }

    let mut bottom_height = 0.0;
    for (index, entry) in panes.iter().enumerate() {
        if entry.descriptor.side != PaneSide::Bottom {
            continue;
        }
        let upper = entry
            .descriptor
            .max_size
            .0
            .min(BOTTOM_ABSOLUTE_MAX_HEIGHT.0)
            .min(BOTTOM_MAX_VIEWPORT_FRACTION * viewport_height)
            .max(entry.descriptor.min_size.0)
            .min(viewport_height);
        let lower = entry.descriptor.min_size.0.min(upper);
        results[index].limits = PaneLimits {
            min: Pixels(lower),
            max: Pixels(upper),
        };
        if entry.is_open() && viewport_height > 0.0 {
            let height = entry.state.size.0.max(lower).min(upper);
            results[index].visible = true;
            results[index].size = Pixels(height);
            bottom_height = height;
        }
    }

    PaneFit {
        panes: results,
        main_width: Pixels((viewport_width - used).max(0.0)),
        main_min: Pixels(main_min_used),
        bottom_height: Pixels(bottom_height),
    }
}
