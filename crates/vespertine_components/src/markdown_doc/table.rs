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
    /// weight).
    pub fn from_content(
        content_widths: &[f32],
        padding: f32,
        min_content: f32,
        min_column: f32,
    ) -> Self {
        Self::from_columns(
            content_widths,
            &vec![0.; content_widths.len()],
            padding,
            min_content,
            min_column,
        )
    }

    /// Geometry with formula columns: `formula_words[c]` is the width of the widest unbreakable
    /// word containing an inline formula in column `c` (0 for none). Neither the content width
    /// nor the floor of such a column is less than that word (the floor adds the padding on
    /// both sides, so the word fits inside the cell).
    pub fn from_columns(
        content_widths: &[f32],
        formula_words: &[f32],
        padding: f32,
        min_content: f32,
        min_column: f32,
    ) -> Self {
        let word = |c: usize| formula_words.get(c).copied().unwrap_or(0.);
        let naturals: Vec<f32> = content_widths
            .iter()
            .enumerate()
            .map(|(c, content)| content.max(word(c)).max(min_content) + 2. * padding)
            .collect();
        let floors: Vec<f32> = naturals
            .iter()
            .enumerate()
            .map(|(c, natural)| {
                let floor = natural.min(min_column);
                if word(c) > 0. {
                    floor.max(word(c) + 2. * padding)
                } else {
                    floor
                }
            })
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

    #[test]
    fn a_formula_word_raises_the_content_width_and_the_floor() {
        let geometry = TableGeometry::from_columns(&[10., 10.], &[0., 150.], 12., 48., 96.);
        assert_eq!(geometry.naturals, [72., 174.]);
        assert_eq!(geometry.floors, [72., 174.]);
        let small = TableGeometry::from_columns(&[10., 10.], &[0., 20.], 12., 48., 96.);
        assert_eq!(small.floors, [72., 72.]);
    }
}
