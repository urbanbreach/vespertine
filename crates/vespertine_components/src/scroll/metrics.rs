//! Overlay rail geometry (CMP-031 R8-R9, R11).
//!
//! One vocabulary serves both orientations: a viewport extent `V`, a maximum scroll `m` and a
//! current scroll position along the axis. The track is the viewport minus a 4 px inset at both
//! ends; the thumb is at least 24 px long and never longer than the track.

/// Track inset at both ends, in logical pixels.
pub const RAIL_TRACK_INSET: f32 = 4.0;
/// Width (height, for a horizontal rail) of the invisible hit strip.
pub const RAIL_HIT_THICKNESS: f32 = 10.0;
/// Resting thumb thickness.
pub const RAIL_THUMB_THICKNESS: f32 = 3.0;
/// Thumb thickness while the strip is hovered or dragged.
pub const RAIL_THUMB_ACTIVE_THICKNESS: f32 = 5.0;
/// The shortest thumb.
pub const RAIL_MIN_THUMB: f32 = 24.0;
/// Distance between the thumb and the outer edge of the viewport.
pub const RAIL_THUMB_EDGE_OFFSET: f32 = 2.0;

/// The orientation of a rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RailOrientation {
    /// A rail at the right edge that scrolls vertically.
    Vertical,
    /// A rail at the bottom edge that scrolls horizontally.
    Horizontal,
}

/// The geometry of the thumb of one rail along its axis.
///
/// Every distance is in logical pixels along the axis. The thumb's distance from the viewport's
/// leading edge is [`RailMetrics::thumb_distance`]; the track starts [`RAIL_TRACK_INSET`] after
/// the viewport's leading edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RailMetrics {
    /// Track length `T = max(V - 2 x inset, 0)`; positive for every existing rail.
    pub track: f32,
    /// Thumb origin measured from the track start.
    pub thumb_start: f32,
    /// Thumb length.
    pub thumb_len: f32,
    /// Maximum scroll distance `m`.
    pub max_scroll: f32,
    /// Thumb travel `max(T - thumb, 0)`.
    pub travel: f32,
}

impl RailMetrics {
    /// The metrics for a viewport extent, a maximum scroll and the current scroll position
    /// (R9). The position clamps to `[0, max_scroll]`, so overscrolled, negative or huge
    /// values land on the ends. `None` when the content fits (`max_scroll <= 0`) or the
    /// viewport leaves no track.
    pub fn new(viewport: f32, max_scroll: f32, scroll: f32) -> Option<Self> {
        if !(viewport.is_finite() && max_scroll.is_finite()) || max_scroll <= 0.0 {
            return None;
        }
        let track = (viewport - 2.0 * RAIL_TRACK_INSET).max(0.0);
        if track <= 0.0 {
            return None;
        }
        let content = viewport + max_scroll;
        let thumb_len = track.min(RAIL_MIN_THUMB.max(track * viewport / content));
        let travel = (track - thumb_len).max(0.0);
        let scroll = if scroll.is_nan() {
            0.0
        } else {
            scroll.clamp(0.0, max_scroll)
        };
        Some(Self {
            track,
            thumb_start: travel * scroll / max_scroll,
            thumb_len,
            max_scroll,
            travel,
        })
    }

    /// The metrics for a viewport and a content extent (`max scroll = content - viewport`).
    pub fn from_extents(viewport: f32, content: f32, scroll: f32) -> Option<Self> {
        Self::new(viewport, content - viewport, scroll)
    }

    /// The thumb's distance from the viewport's leading edge: the track inset plus its origin.
    pub fn thumb_distance(&self) -> f32 {
        RAIL_TRACK_INSET + self.thumb_start
    }

    /// The grab offset of a press at `pointer` (a position in the track: the window coordinate
    /// minus the track's start): the pointer's distance from the thumb start when it is within
    /// the thumb, else half the thumb length so the thumb is centered under the pointer (R11).
    pub fn grab_offset(&self, pointer: f32) -> f32 {
        if pointer >= self.thumb_start && pointer <= self.thumb_start + self.thumb_len {
            pointer - self.thumb_start
        } else {
            self.thumb_len / 2.0
        }
    }

    /// The scroll fraction (`0..=1`) the drag target for `pointer` and `grab` means (R11): the
    /// thumb origin `pointer - grab` clamps to the travel; a full-track thumb (no travel) always
    /// targets the start.
    pub fn target_fraction(&self, pointer: f32, grab: f32) -> f32 {
        if self.travel <= 0.0 {
            return 0.0;
        }
        (pointer - grab).clamp(0.0, self.travel) / self.travel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_overlay_metric_tables() {
        // Tiny tracks give no rail.
        assert!(RailMetrics::new(8.0, 100.0, 0.0).is_none());
        assert!(RailMetrics::new(0.0, 100.0, 0.0).is_none());
        assert!(RailMetrics::new(-5.0, 100.0, 0.0).is_none());
        // (V, C) = (320, 960), scroll 0.
        let m = RailMetrics::from_extents(320.0, 960.0, 0.0).unwrap();
        assert_eq!(m.track, 312.0);
        assert_eq!(m.thumb_len, 104.0);
        assert_eq!(m.thumb_start, 0.0);
        assert_eq!(m.travel, 208.0);
        // Scroll 640 (the maximum): origin = travel.
        let m = RailMetrics::from_extents(320.0, 960.0, 640.0).unwrap();
        assert_eq!(m.thumb_start, 208.0);
        assert_eq!(m.thumb_distance(), 212.0);
        // Huge content gives a 24 px thumb.
        let m = RailMetrics::from_extents(100.0, 10000.0, 0.0).unwrap();
        assert_eq!(m.thumb_len, 24.0);
        // Overscrolled offsets clamp to the ends.
        let low = RailMetrics::new(100.0, 9900.0, -3.0).unwrap();
        let high = RailMetrics::new(100.0, 9900.0, 99999.0).unwrap();
        assert_eq!(low.thumb_start, 0.0);
        assert_eq!(high.thumb_start, high.travel);
        // Content that fits gives no rail.
        assert!(RailMetrics::from_extents(320.0, 320.0, 0.0).is_none());
        assert!(RailMetrics::from_extents(320.0, 100.0, 0.0).is_none());
    }

    #[test]
    fn a_track_shorter_than_the_minimum_uses_the_whole_track() {
        let m = RailMetrics::new(20.0, 500.0, 0.0).unwrap();
        assert_eq!(m.track, 12.0);
        assert_eq!(m.thumb_len, 12.0);
        assert_eq!(m.travel, 0.0);
        assert_eq!(m.target_fraction(7.0, 3.0), 0.0);
    }

    #[test]
    fn a5_press_and_drag_targets() {
        let m = RailMetrics::from_extents(320.0, 960.0, 320.0).unwrap();
        // The thumb starts at 104 (travel 208, half scrolled); a press 10 px into it keeps 10 px.
        let pointer = m.thumb_start + 10.0;
        let grab = m.grab_offset(pointer);
        assert_eq!(grab, 10.0);
        assert!((m.target_fraction(pointer, grab) - 0.5).abs() < 1e-6);
        // Pressing the track centers the thumb under the pointer.
        let pointer = 20.0;
        let grab = m.grab_offset(pointer);
        assert_eq!(grab, m.thumb_len / 2.0);
        assert!(
            (m.target_fraction(pointer, grab) - (20.0 - 52.0f32).max(0.0) / 208.0).abs() < 1e-6
        );
        // Dragging past either end clamps.
        assert_eq!(m.target_fraction(-500.0, grab), 0.0);
        assert_eq!(m.target_fraction(5000.0, grab), 1.0);
    }
}
