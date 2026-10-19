//! Keyboard stepping over a list whose rows may be unavailable (CMP-002 R23).
//!
//! Everything is index based over a predicate (or a `&[bool]` slice), so menus, selects,
//! comboboxes and pickers share one rule: order is the mounted order, an active index moves by
//! one with wraparound, disabled rows are skipped in the stepping direction, Home and End jump to
//! the first and last available row, and a list without any available row stays without an
//! active row.

/// Direction of one keyboard step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepDirection {
    /// Down / Ctrl-N.
    Forward,
    /// Up / Ctrl-P.
    Backward,
}

/// A list end for Home and End.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Home: the first available row.
    First,
    /// End: the last available row.
    Last,
}

/// Steps from `active` through a list of `len` rows, skipping rows for which `available`
/// answers false.
///
/// With no active row (or an index outside the list) forward enters at the first available row
/// and backward at the last. Returns `None` when `len` is 0 or no row is available. A single
/// available row that is already active is returned again.
pub fn step_available(
    len: usize,
    active: Option<usize>,
    direction: StepDirection,
    available: impl Fn(usize) -> bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let active = active.filter(|index| *index < len);
    let start = match (active, direction) {
        (Some(index), _) => index,
        (None, StepDirection::Forward) => len - 1,
        (None, StepDirection::Backward) => 0,
    };
    let mut index = start;
    for _ in 0..len {
        index = match direction {
            StepDirection::Forward => (index + 1) % len,
            StepDirection::Backward => (index + len - 1) % len,
        };
        if available(index) {
            return Some(index);
        }
    }
    None
}

/// [`step_available`] over a slice of availability flags (`true` = the row can take the
/// highlight).
pub fn step_slice(
    available: &[bool],
    active: Option<usize>,
    direction: StepDirection,
) -> Option<usize> {
    step_available(available.len(), active, direction, |index| available[index])
}

/// Home / End: the first or last available row of `len` rows.
pub fn edge_available(len: usize, edge: Edge, available: impl Fn(usize) -> bool) -> Option<usize> {
    match edge {
        Edge::First => (0..len).find(|index| available(*index)),
        Edge::Last => (0..len).rev().find(|index| available(*index)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use StepDirection::{Backward, Forward};

    #[test]
    fn wraps_both_ways() {
        let all = [true; 4];
        assert_eq!(step_slice(&all, Some(3), Forward), Some(0));
        assert_eq!(step_slice(&all, Some(0), Backward), Some(3));
        assert_eq!(step_slice(&all, Some(1), Forward), Some(2));
        assert_eq!(step_slice(&all, Some(2), Backward), Some(1));
    }

    #[test]
    fn entering_from_no_active_row() {
        let all = [true; 3];
        assert_eq!(step_slice(&all, None, Forward), Some(0));
        assert_eq!(step_slice(&all, None, Backward), Some(2));
        // An index outside the list counts as no active row.
        assert_eq!(step_slice(&all, Some(9), Forward), Some(0));
    }

    #[test]
    fn disabled_rows_are_skipped_in_the_stepping_direction() {
        let rows = [true, false, false, true, false];
        assert_eq!(step_slice(&rows, Some(0), Forward), Some(3));
        assert_eq!(step_slice(&rows, Some(3), Forward), Some(0));
        assert_eq!(step_slice(&rows, Some(3), Backward), Some(0));
        assert_eq!(step_slice(&rows, Some(0), Backward), Some(3));
        assert_eq!(step_slice(&[false, true], None, Forward), Some(1));
        assert_eq!(step_slice(&[true, false], None, Backward), Some(0));
    }

    #[test]
    fn empty_and_all_disabled_lists_stay_without_active_row() {
        assert_eq!(step_slice(&[], None, Forward), None);
        assert_eq!(step_slice(&[], Some(0), Backward), None);
        let off = [false; 3];
        assert_eq!(step_slice(&off, None, Forward), None);
        assert_eq!(step_slice(&off, Some(1), Backward), None);
    }

    #[test]
    fn a_lone_available_row_stays_active() {
        assert_eq!(step_slice(&[false, true], Some(1), Forward), Some(1));
        assert_eq!(step_slice(&[false, true], Some(1), Backward), Some(1));
    }

    #[test]
    fn home_and_end_jump_to_available_rows() {
        let rows = [false, true, true, false];
        assert_eq!(edge_available(4, Edge::First, |i| rows[i]), Some(1));
        assert_eq!(edge_available(4, Edge::Last, |i| rows[i]), Some(2));
        assert_eq!(edge_available(0, Edge::First, |_| true), None);
        assert_eq!(edge_available(3, Edge::Last, |_| false), None);
    }
}
