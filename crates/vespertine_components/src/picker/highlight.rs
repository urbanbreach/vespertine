//! The highlighted row of a picker, kept by identity (CMP-006 R13).

use super::results::RowKey;
use crate::floating::stepping::{Edge, StepDirection, edge_available, step_slice};

/// A highlight that follows its row's key, remembering the last flat index.
///
/// Results arriving around the highlighted row never move it; when the row is no longer listed
/// the highlight falls to the remembered index clamped to the row count; with no highlight yet
/// the first available row is the highlight, so a palette has no empty-highlight state: the first
/// Down moves from the first row to the second and the first Up wraps to the last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Highlight {
    key: Option<RowKey>,
    last_index: usize,
}

impl Highlight {
    /// A highlight with nothing remembered.
    pub fn new() -> Self {
        Self::default()
    }

    /// A highlight that already follows `key` (restoring an identity).
    pub fn seeded(key: RowKey) -> Self {
        Self {
            key: Some(key),
            last_index: 0,
        }
    }

    /// The highlighted key, as of the last reconcile.
    pub fn key(&self) -> Option<&RowKey> {
        self.key.as_ref()
    }

    /// The last flat index of the highlighted row.
    pub fn last_index(&self) -> usize {
        self.last_index
    }

    /// Forgets the highlight (tab or query change): the first row is highlighted again.
    pub fn reset(&mut self) {
        self.key = None;
        self.last_index = 0;
    }

    /// Re-resolves the highlight against the current flat rows and returns its flat index
    /// (`None` when no row can take it).
    pub fn reconcile(&mut self, keys: &[RowKey], enabled: &[bool]) -> Option<usize> {
        debug_assert_eq!(keys.len(), enabled.len());
        if keys.is_empty() {
            self.key = None;
            return None;
        }
        let index = match self.key.as_ref() {
            Some(key) => keys
                .iter()
                .position(|candidate| candidate == key)
                .filter(|ix| enabled[*ix])
                .or_else(|| self.fallback(keys.len(), enabled)),
            None => edge_available(keys.len(), Edge::First, |ix| enabled[ix]),
        };
        self.apply(keys, index)
    }

    fn fallback(&self, len: usize, enabled: &[bool]) -> Option<usize> {
        let clamped = self.last_index.min(len - 1);
        if enabled[clamped] {
            Some(clamped)
        } else {
            // The remembered row cannot take the highlight: the next available row after it.
            crate::floating::stepping::step_available(
                len,
                Some(clamped),
                StepDirection::Forward,
                |ix| enabled[ix],
            )
        }
    }

    fn apply(&mut self, keys: &[RowKey], index: Option<usize>) -> Option<usize> {
        match index {
            Some(ix) => {
                self.key = Some(keys[ix].clone());
                self.last_index = ix;
            }
            None => self.key = None,
        }
        index
    }

    /// Up or Down (Ctrl-P / Ctrl-N) with wraparound, skipping disabled rows (CMP-002 R23).
    pub fn step(
        &mut self,
        keys: &[RowKey],
        enabled: &[bool],
        direction: StepDirection,
    ) -> Option<usize> {
        let current = self.reconcile(keys, enabled);
        let next = step_slice(enabled, current, direction);
        self.apply(keys, next)
    }

    /// Home or End.
    pub fn jump(&mut self, keys: &[RowKey], enabled: &[bool], edge: Edge) -> Option<usize> {
        let next = edge_available(keys.len(), edge, |ix| enabled[ix]);
        self.apply(keys, next)
    }

    /// Hover: the pointer moved over `key` (real motion only, CMP-002 R24).
    pub fn point_at(&mut self, keys: &[RowKey], enabled: &[bool], key: &RowKey) -> Option<usize> {
        let index = keys
            .iter()
            .position(|candidate| candidate == key)
            .filter(|ix| enabled[*ix]);
        if index.is_some() {
            self.apply(keys, index);
        }
        index
    }
}

/// Where a row sits in the scrolled content, for [`reveal_offset`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowExtent {
    /// Top of the row in content coordinates.
    pub top: f32,
    /// Bottom of the row in content coordinates.
    pub bottom: f32,
    /// Top of the row's section header when the row is the first of its section.
    pub header_top: Option<f32>,
    /// The row is the very first row of the list.
    pub first_overall: bool,
}

/// The scroll offset (distance from the content top, non-negative) that reveals a row after a
/// keyboard step, using the nearest-edge strategy (R13): the offset only moves as far as needed;
/// the first row of a section brings its header into view with it, and the very first row returns
/// the offset to zero so a header never hides above the edge.
pub fn reveal_offset(offset: f32, viewport_height: f32, row: RowExtent) -> f32 {
    if row.first_overall {
        return 0.0;
    }
    let top = row.header_top.unwrap_or(row.top).min(row.top);
    if top < offset {
        top
    } else if row.bottom > offset + viewport_height {
        (row.bottom - viewport_height).max(0.0).min(top)
    } else {
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use StepDirection::{Backward, Forward};

    fn keys(names: &[&str]) -> Vec<RowKey> {
        names
            .iter()
            .map(|name| RowKey::from(name.to_string()))
            .collect()
    }

    #[test]
    fn the_first_row_is_the_highlight_so_the_first_down_moves_to_the_second() {
        let rows = keys(&["a", "b", "c"]);
        let on = [true; 3];
        let mut highlight = Highlight::new();
        assert_eq!(highlight.reconcile(&rows, &on), Some(0));
        assert_eq!(highlight.step(&rows, &on, Forward), Some(1));

        let mut highlight = Highlight::new();
        assert_eq!(
            highlight.step(&rows, &on, Backward),
            Some(2),
            "the first Up wraps"
        );
    }

    #[test]
    fn rows_arriving_before_the_highlight_never_move_it() {
        let mut highlight = Highlight::new();
        let on3 = [true; 3];
        highlight.reconcile(&keys(&["a", "b", "c"]), &on3);
        highlight.step(&keys(&["a", "b", "c"]), &on3, Forward);
        assert_eq!(highlight.key().map(|k| k.as_ref()), Some("b"));
        let arrived = keys(&["x", "y", "a", "b", "c"]);
        assert_eq!(highlight.reconcile(&arrived, &[true; 5]), Some(3));
        assert_eq!(highlight.key().map(|k| k.as_ref()), Some("b"));
    }

    #[test]
    fn a_vanished_row_falls_to_the_remembered_index_clamped() {
        let mut highlight = Highlight::new();
        let rows = keys(&["a", "b", "c", "d"]);
        highlight.reconcile(&rows, &[true; 4]);
        highlight.jump(&rows, &[true; 4], Edge::Last);
        assert_eq!(highlight.last_index(), 3);
        // The highlighted row disappears; the list shrank below the remembered index.
        assert_eq!(highlight.reconcile(&keys(&["a", "b"]), &[true; 2]), Some(1));
        // In the middle: the row at the remembered index takes over.
        let mut highlight = Highlight::new();
        highlight.reconcile(&rows, &[true; 4]);
        highlight.step(&rows, &[true; 4], Forward);
        assert_eq!(
            highlight.reconcile(&keys(&["a", "c", "d"]), &[true; 3]),
            Some(1)
        );
        assert_eq!(highlight.key().map(|k| k.as_ref()), Some("c"));
    }

    #[test]
    fn stepping_wraps_and_skips_disabled_rows() {
        let rows = keys(&["a", "b", "c"]);
        let enabled = [true, false, true];
        let mut highlight = Highlight::new();
        assert_eq!(highlight.step(&rows, &enabled, Forward), Some(2));
        assert_eq!(highlight.step(&rows, &enabled, Forward), Some(0));
        assert_eq!(highlight.step(&rows, &enabled, Backward), Some(2));
        assert_eq!(highlight.jump(&rows, &enabled, Edge::First), Some(0));
    }

    #[test]
    fn an_empty_list_has_no_highlight_and_reset_returns_to_the_first_row() {
        let mut highlight = Highlight::new();
        assert_eq!(highlight.reconcile(&[], &[]), None);
        let rows = keys(&["a", "b"]);
        highlight.step(&rows, &[true; 2], Forward);
        highlight.step(&rows, &[true; 2], Forward);
        highlight.reset();
        assert_eq!(highlight.reconcile(&rows, &[true; 2]), Some(0));
    }

    #[test]
    fn hover_moves_the_highlight_to_an_enabled_row_only() {
        let rows = keys(&["a", "b"]);
        let enabled = [true, false];
        let mut highlight = Highlight::new();
        highlight.reconcile(&rows, &enabled);
        assert_eq!(highlight.point_at(&rows, &enabled, &"b".into()), None);
        assert_eq!(highlight.key().map(|k| k.as_ref()), Some("a"));
        assert_eq!(highlight.point_at(&rows, &[true; 2], &"b".into()), Some(1));
    }

    #[test]
    fn reveal_uses_the_nearest_edge_and_brings_headers_and_the_top_into_view() {
        let row = |top: f32| RowExtent {
            top,
            bottom: top + 30.0,
            header_top: None,
            first_overall: false,
        };
        // Already visible: unchanged.
        assert_eq!(reveal_offset(100.0, 200.0, row(150.0)), 100.0);
        // Below the viewport: the row's bottom meets the bottom edge.
        assert_eq!(reveal_offset(0.0, 200.0, row(250.0)), 80.0);
        // Above the viewport: the row's top meets the top edge.
        assert_eq!(reveal_offset(300.0, 200.0, row(100.0)), 100.0);
        // The first row of a section brings its header into view with it.
        let first_in_section = RowExtent {
            header_top: Some(90.0),
            ..row(120.0)
        };
        assert_eq!(reveal_offset(100.0, 200.0, first_in_section), 90.0);
        // The very first row returns the offset to zero.
        let very_first = RowExtent {
            first_overall: true,
            ..row(24.0)
        };
        assert_eq!(reveal_offset(40.0, 200.0, very_first), 0.0);
    }
}
