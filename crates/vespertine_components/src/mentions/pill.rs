//! Pill geometry and vertical centering of chips (CMP-010 R6, R7).
//!
//! Pure functions over plain numbers (logical pixels in window coordinates) so the painting code
//! only has to turn the results into quads.

/// Vertical inset of the pill inside its row box, top and bottom.
pub const PILL_VERTICAL_INSET: f32 = 1.5;
/// Corner radius of the pill.
pub const PILL_RADIUS: f32 = 5.0;
/// Opacity of the "ink" overlay that fills the pill.
pub const PILL_FILL_OPACITY: f32 = 0.09;
/// Inset of the icon well from the pill's top-left corner.
pub const ICON_WELL_INSET: f32 = 1.0;
/// Corner radius of the icon well.
pub const ICON_WELL_RADIUS: f32 = 4.0;
/// Largest icon size.
pub const ICON_MAX_SIZE: f32 = 14.0;
/// The icon is at most the well side minus this much.
pub const ICON_WELL_MARGIN: f32 = 4.0;
/// The centering offset never exceeds this magnitude.
pub const CENTERING_CLAMP: f32 = 1.5;

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillRect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl PillRect {
    /// A rectangle from its origin and size.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// The icon well of a chip's first segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconWell {
    /// The well square.
    pub bounds: PillRect,
    /// Corner radius of the well.
    pub radius: f32,
    /// The icon square, centered in the well.
    pub icon: PillRect,
}

/// The geometry of one chip row segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillGeometry {
    /// The pill: the row box inset vertically and shifted by the centering offset.
    pub bounds: PillRect,
    /// Corner radius of the pill.
    pub radius: f32,
    /// The icon well; present only on a chip's first segment.
    pub well: Option<IconWell>,
}

/// Computes the pill of a segment (R6). `row` is the segment's row box (x range of the segment,
/// the full line box vertically), `centering_offset` the result of [`centering_offset`], and
/// `first_segment` whether the segment carries the icon.
pub fn pill_geometry(row: PillRect, centering_offset: f32, first_segment: bool) -> PillGeometry {
    let bounds = PillRect::new(
        row.x,
        row.y + PILL_VERTICAL_INSET + centering_offset,
        row.width,
        (row.height - 2.0 * PILL_VERTICAL_INSET).max(0.0),
    );
    let well = first_segment.then(|| {
        let side = (bounds.height - 2.0 * ICON_WELL_INSET).max(0.0);
        let well = PillRect::new(
            bounds.x + ICON_WELL_INSET,
            bounds.y + ICON_WELL_INSET,
            side,
            side,
        );
        let icon_side = ICON_MAX_SIZE.min(side - ICON_WELL_MARGIN).max(0.0);
        let icon = PillRect::new(
            well.x + (side - icon_side) / 2.0,
            well.y + (side - icon_side) / 2.0,
            icon_side,
            icon_side,
        );
        IconWell {
            bounds: well,
            radius: ICON_WELL_RADIUS,
            icon,
        }
    });
    PillGeometry {
        bounds,
        radius: PILL_RADIUS,
        well,
    }
}

/// Metrics of the face that actually shaped a label (fallbacks included).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceMetrics {
    /// Distance from the baseline to the top of the line box content (positive).
    pub ascent: f32,
    /// Distance from the baseline to the bottom of the line box content (positive).
    pub descent: f32,
    /// Cap height, if the face has one.
    pub cap_height: Option<f32>,
}

/// Converts `value` to device pixels at `scale`, rounds to nearest with exact halves toward zero,
/// and converts back (R7).
pub fn snap_to_device(value: f32, scale: f32) -> f32 {
    let device = value * scale;
    let floor = device.floor();
    let fraction = device - floor;
    let rounded = if fraction > 0.5 {
        floor + 1.0
    } else if fraction < 0.5 {
        floor
    } else if device > 0.0 {
        floor
    } else {
        floor + 1.0
    };
    rounded / scale
}

/// The vertical offset that centers the pill on the label's cap height instead of the row middle
/// (R7): `snap(baseline - cap_height / 2 - (row_top + line_height / 2))` with
/// `baseline = snap(row_top + (line_height - ascent - descent) / 2 + ascent)`, clamped to
/// ±1.5 px; a face without a cap height gets 0. All values are in window coordinates because the
/// rounding depends on where the row lands; `scale` is the window scale factor.
pub fn centering_offset(row_top: f32, line_height: f32, face: FaceMetrics, scale: f32) -> f32 {
    let Some(cap_height) = face.cap_height else {
        return 0.0;
    };
    let baseline = snap_to_device(
        row_top + (line_height - face.ascent - face.descent) / 2.0 + face.ascent,
        scale,
    );
    let offset = snap_to_device(
        baseline - cap_height / 2.0 - (row_top + line_height / 2.0),
        scale,
    );
    offset.clamp(-CENTERING_CLAMP, CENTERING_CLAMP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(ascent: f32, descent: f32, cap: Option<f32>) -> FaceMetrics {
        FaceMetrics {
            ascent,
            descent,
            cap_height: cap,
        }
    }

    #[test]
    fn halves_round_toward_zero() {
        assert_eq!(snap_to_device(2.5, 1.0), 2.0);
        assert_eq!(snap_to_device(-2.5, 1.0), -2.0);
        assert_eq!(snap_to_device(2.51, 1.0), 3.0);
        assert_eq!(snap_to_device(2.49, 1.0), 2.0);
        assert_eq!(snap_to_device(-2.51, 1.0), -3.0);
        assert_eq!(snap_to_device(0.25, 2.0), 0.0);
        assert_eq!(snap_to_device(0.3, 2.0), 0.5);
        assert_eq!(snap_to_device(0.75, 2.0), 0.5);
        assert_eq!(snap_to_device(1.25, 2.0), 1.0);
        assert_eq!(snap_to_device(-0.75, 2.0), -0.5);
    }

    #[test]
    fn offset_is_zero_when_cap_height_equals_ascent_minus_descent() {
        let metrics = face(16.0, 4.0, Some(12.0));
        for scale in [1.0, 2.0] {
            for row_top in [0.0, 10.0, 37.0, 100.5] {
                assert_eq!(
                    centering_offset(row_top, 20.0, metrics, scale),
                    0.0,
                    "{row_top} {scale}"
                );
            }
        }
    }

    #[test]
    fn offset_without_cap_height_is_zero() {
        assert_eq!(
            centering_offset(10.0, 20.0, face(16.0, 4.0, None), 2.0),
            0.0
        );
    }

    #[test]
    fn offset_follows_the_cap_height_and_snaps_at_scale_two() {
        // baseline = 10 + (20 - 14 - 4) / 2 + 14 = 25; offset = 25 - 5 - 20 = 0 for cap 10.
        assert_eq!(
            centering_offset(10.0, 20.0, face(14.0, 4.0, Some(10.0)), 2.0),
            0.0
        );
        // cap 9: 25 - 4.5 - 20 = 0.5 (a device pixel at scale 2).
        assert_eq!(
            centering_offset(10.0, 20.0, face(14.0, 4.0, Some(9.0)), 2.0),
            0.5
        );
        // cap 8.6: 0.7 snaps to 0.5 at scale 2 but to 1.0 at scale 1.
        assert_eq!(
            centering_offset(10.0, 20.0, face(14.0, 4.0, Some(8.6)), 2.0),
            0.5
        );
        assert_eq!(
            centering_offset(10.0, 20.0, face(14.0, 4.0, Some(8.6)), 1.0),
            1.0
        );
        // cap 11: -0.5.
        assert_eq!(
            centering_offset(10.0, 20.0, face(14.0, 4.0, Some(11.0)), 2.0),
            -0.5
        );
    }

    #[test]
    fn offset_is_clamped_to_one_and_a_half_pixels() {
        assert_eq!(
            centering_offset(0.0, 20.0, face(14.0, 4.0, Some(2.0)), 2.0),
            1.5
        );
        assert_eq!(
            centering_offset(0.0, 20.0, face(18.0, 2.0, Some(30.0)), 2.0),
            -1.5
        );
    }

    #[test]
    fn offset_stays_within_the_clamp_for_a_range_of_faces() {
        for cap in [0.0f32, 4.0, 7.3, 9.0, 12.0, 15.9, 40.0] {
            for row_top in [0.0f32, 3.3, 19.75] {
                let offset = centering_offset(row_top, 22.0, face(15.0, 5.0, Some(cap)), 2.0);
                assert!(offset.abs() <= 1.5);
                assert_eq!(
                    (offset * 2.0).fract(),
                    0.0,
                    "device pixel multiple at scale 2"
                );
            }
        }
    }

    #[test]
    fn pill_is_inset_and_shifted_by_the_offset() {
        let row = PillRect::new(30.0, 100.0, 80.0, 20.0);
        let geometry = pill_geometry(row, 0.5, false);
        assert_eq!(geometry.bounds, PillRect::new(30.0, 102.0, 80.0, 17.0));
        assert_eq!(geometry.radius, 5.0);
        assert!(geometry.well.is_none());
    }

    #[test]
    fn first_segment_has_a_centered_icon_well() {
        let row = PillRect::new(30.0, 100.0, 80.0, 20.0);
        let geometry = pill_geometry(row, 0.0, true);
        let well = geometry.well.expect("first segment has a well");
        // Pill height 17, well side 15 inset by 1 px, icon min(14, 15 - 4) = 11 centered.
        assert_eq!(well.bounds, PillRect::new(31.0, 102.5, 15.0, 15.0));
        assert_eq!(well.radius, 4.0);
        assert_eq!(well.icon, PillRect::new(33.0, 104.5, 11.0, 11.0));
    }

    #[test]
    fn icon_is_capped_at_fourteen_pixels() {
        let row = PillRect::new(0.0, 0.0, 80.0, 40.0);
        let well = pill_geometry(row, 0.0, true).well.unwrap();
        assert_eq!(well.icon.width, 14.0);
        assert_eq!(well.icon.height, 14.0);
        assert_eq!(
            well.icon.x,
            well.bounds.x + (well.bounds.width - 14.0) / 2.0
        );
    }

    #[test]
    fn tiny_rows_do_not_produce_negative_sizes() {
        let geometry = pill_geometry(PillRect::new(0.0, 0.0, 10.0, 2.0), 0.0, true);
        assert_eq!(geometry.bounds.height, 0.0);
        let well = geometry.well.unwrap();
        assert_eq!(well.bounds.width, 0.0);
        assert_eq!(well.icon.width, 0.0);
    }
}
