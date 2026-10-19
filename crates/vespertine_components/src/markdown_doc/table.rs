//! Table column geometry (CMP-011 R14): natural widths, floors and the minimum table width from
//! measured content widths.

/// Column geometry of one table.
#[derive(Clone, Debug, PartialEq)]
pub struct TableGeometry {
    /// Natural width of each column: content width (floored at the minimum content width) plus
    /// the padding on both sides.
    pub naturals: Vec<f32>,
    /// Floor of each column: `min(natural, min column width)`.
    pub floors: Vec<f32>,
    /// The table's minimum width: the sum of the floors.
    pub min_width: f32,
}

impl TableGeometry {
    /// Geometry from the maximum unwrapped content width of each column (header at its bold
    /// weight; for a column holding a formula the caller passes at least the widest unbreakable
    /// word containing it).
    pub fn from_content(
        content_widths: &[f32],
        padding: f32,
        min_content: f32,
        min_column: f32,
    ) -> Self {
        let naturals: Vec<f32> = content_widths
            .iter()
            .map(|content| content.max(min_content) + 2. * padding)
            .collect();
        let floors: Vec<f32> = naturals
            .iter()
            .map(|natural| natural.min(min_column))
            .collect();
        let min_width = floors.iter().sum();
        Self {
            naturals,
            floors,
            min_width,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naturals_floors_and_minimum_width() {
        let geometry = TableGeometry::from_content(&[10., 200., 60.], 12., 48., 96.);
        assert_eq!(geometry.naturals, [72., 224., 84.]);
        assert_eq!(geometry.floors, [72., 96., 84.]);
        assert_eq!(geometry.min_width, 252.);
    }
}
