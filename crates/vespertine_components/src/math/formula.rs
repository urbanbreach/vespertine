//! Laid-out formulas in em units and the math style settings (CMP-012 R1, R2, R12).

use super::faces::{MathFace, face_data};
use super::parse::MathError;

/// One positioned item of a display list. Coordinates are in ems of the surrounding text (1.0 em
/// is the font size of the text the formula sits in); `y` grows downwards from the baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DisplayItem {
    /// A glyph of a face of the face table, with its origin (baseline, left) and the size of the
    /// glyph in ems (script glyphs are smaller than 1.0).
    Glyph {
        /// Index into [`super::FACE_TABLE`].
        font: u8,
        /// Glyph identifier in that face.
        glyph: u16,
        /// Horizontal origin.
        x: f32,
        /// Baseline position, downwards from the formula baseline.
        y: f32,
        /// Font size of the glyph in ems.
        scale: f32,
    },
    /// A filled rectangle (fraction bar, radical bar, overline).
    Rule {
        x: f32,
        /// Top edge, downwards from the formula baseline.
        y: f32,
        width: f32,
        height: f32,
    },
}

/// A formula laid out once in ems: scaling to a font size never lays it out again.
#[derive(Clone, Debug, PartialEq)]
pub struct Formula {
    items: Vec<DisplayItem>,
    advance: f32,
    ascent: f32,
    descent: f32,
    ink_left: f32,
    ink_right: f32,
}

impl Formula {
    /// A formula from its display list; the painted extent is computed from the glyph ink boxes
    /// of the faces and the rules. Fails for non-finite numbers and for glyphs of unknown faces.
    pub fn from_items(
        items: Vec<DisplayItem>,
        advance: f32,
        ascent: f32,
        descent: f32,
    ) -> Result<Formula, MathError> {
        if ![advance, ascent, descent].iter().all(|v| v.is_finite()) {
            return Err(MathError::NonFinite);
        }
        let mut ink_left = 0.0f32.min(advance);
        let mut ink_right = 0.0f32.max(advance);
        for item in &items {
            let (left, right) = match *item {
                DisplayItem::Glyph {
                    font,
                    glyph,
                    x,
                    scale,
                    ..
                } => {
                    let data = face_data(usize::from(font)).ok_or(MathError::NonFinite)?;
                    let bounds = data.bounds(ttf_parser::GlyphId(glyph));
                    (x + bounds.x_min * scale, x + bounds.x_max * scale)
                }
                DisplayItem::Rule { x, width, .. } => (x, x + width),
            };
            if !(left.is_finite() && right.is_finite()) {
                return Err(MathError::NonFinite);
            }
            ink_left = ink_left.min(left);
            ink_right = ink_right.max(right);
        }
        Ok(Formula {
            items,
            advance,
            ascent,
            descent,
            ink_left,
            ink_right,
        })
    }

    /// The display list.
    pub fn items(&self) -> &[DisplayItem] {
        &self.items
    }

    /// Advance width in ems.
    pub fn advance(&self) -> f32 {
        self.advance
    }

    /// Height above the baseline in ems.
    pub fn ascent(&self) -> f32 {
        self.ascent
    }

    /// Depth below the baseline in ems.
    pub fn descent(&self) -> f32 {
        self.descent
    }

    /// Ascent plus descent in ems.
    pub fn height(&self) -> f32 {
        self.ascent + self.descent
    }

    /// The horizontal extent in ems: the advance box widened to everything the formula paints
    /// (every glyph's ink and every rule). O(1): it is computed when the formula is built.
    pub fn extent(&self) -> f32 {
        self.ink_right - self.ink_left
    }

    /// Where the painted extent starts relative to the origin (never positive).
    pub fn extent_left(&self) -> f32 {
        self.ink_left
    }

    /// The cache weight of an entry: source bytes plus items times the item size (CMP-012 R10).
    pub fn weight(&self, source_len: usize) -> usize {
        source_len + self.items.len() * std::mem::size_of::<DisplayItem>()
    }
}

/// Smallest accepted math size in percent.
pub const MIN_SIZE_PERCENT: u16 = 75;
/// Largest accepted math size in percent.
pub const MAX_SIZE_PERCENT: u16 = 200;
/// Default math size in percent.
pub const DEFAULT_SIZE_PERCENT: u16 = 100;

/// Math settings of a document (CMP-012 R2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MathStyle {
    /// Whether formulas are typeset (default true); disabled formulas show as code.
    pub enabled: bool,
    /// The selected face (default: the default sans face).
    pub face: MathFace,
    size_percent: u16,
}

impl Default for MathStyle {
    fn default() -> Self {
        MathStyle {
            enabled: true,
            face: MathFace::DEFAULT,
            size_percent: DEFAULT_SIZE_PERCENT,
        }
    }
}

impl MathStyle {
    /// The default style.
    pub fn new() -> Self {
        Self::default()
    }

    /// With the given enabled flag.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// With the given face.
    pub fn face(mut self, face: MathFace) -> Self {
        self.face = face;
        self
    }

    /// With the given size in percent of the surrounding text size, clamped to 75..=200.
    pub fn size_percent(mut self, percent: i32) -> Self {
        self.set_size_percent(percent);
        self
    }

    /// Sets the size in percent, clamping to 75..=200.
    pub fn set_size_percent(&mut self, percent: i32) {
        self.size_percent =
            percent.clamp(i32::from(MIN_SIZE_PERCENT), i32::from(MAX_SIZE_PERCENT)) as u16;
    }

    /// The size in percent (always within 75..=200).
    pub fn size(&self) -> u16 {
        self.size_percent
    }

    /// The formula scale: size / 100.
    pub fn scale(&self) -> f32 {
        f32::from(self.size_percent) / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_style_clamps_the_size_and_scales_by_a_hundredth() {
        let style = MathStyle::new();
        assert!(style.enabled);
        assert_eq!(style.face, MathFace::DEFAULT);
        assert_eq!(style.size(), 100);
        assert_eq!(style.scale(), 1.0);
        assert_eq!(MathStyle::new().size_percent(10).size(), 75);
        assert_eq!(MathStyle::new().size_percent(1000).size(), 200);
        assert_eq!(MathStyle::new().size_percent(150).scale(), 1.5);
        assert_eq!(MathStyle::new().size_percent(-3).size(), 75);
    }

    #[test]
    fn the_extent_widens_the_advance_box_to_rules_and_overhangs() {
        let rules = vec![DisplayItem::Rule {
            x: -0.5,
            y: -1.0,
            width: 3.0,
            height: 0.1,
        }];
        let formula = Formula::from_items(rules, 1.0, 1.0, 0.0).unwrap();
        assert_eq!(formula.extent_left(), -0.5);
        assert_eq!(formula.extent(), 3.0);
        assert!(Formula::from_items(Vec::new(), f32::NAN, 0.0, 0.0).is_err());
    }
}
