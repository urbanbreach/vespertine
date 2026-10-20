//! The fixed math face table and the OpenType MATH data read from it (CMP-012 R1, R2, R11).
//!
//! A face is identified by its index in [`FACE_TABLE`]; a glyph in a display list names the face
//! that produced it by that index, whichever [`MathFace`] was selected. Faces are parsed on first
//! use only: a face nobody lays out is never parsed, and a face that only backs glyphs another
//! face lacks is parsed when such a glyph occurs.
//!
//! # Faces
//!
//! Every face carries a complete OpenType MATH table (constants, glyph variants and assemblies),
//! is bundled unmodified from its original upstream release and is available under a permissive
//! license (the SIL Open Font License 1.1 or the GUST Font License); `THIRD_PARTY.md` records the
//! source, version and license text of each. The indices are stable: new faces are appended.
//!
//! | Index | Face | Style | License |
//! | --- | --- | --- | --- |
//! | 0 | Noto Sans Math 3.000 | the default sans face, beside UI sans-serifs | OFL-1.1 |
//! | 1 | STIX Two Math 2.13 | Times-style serif covering every glyph; backs glyphs missing from the others | OFL-1.1 |
//! | 2 | Latin Modern Math 1.959 | classic LaTeX (Computer Modern) | GUST Font License |
//! | 3 | Libertinus Math 7.051 | warm old-style book serif | OFL-1.1 |
//! | 4 | TeX Gyre Pagella Math 1.632 | calligraphic-humanist book serif (Palatino style) | GUST Font License |
//! | 5 | Euler Math 0.75 | upright handwritten style (Zapf's Euler) | OFL-1.1 |
//!
//! The license texts are in `fonts/`. Adding a face means appending one [`FaceInfo`] row and one
//! [`MathFace`] constant; nothing else in the engine depends on the number of faces.

use std::sync::OnceLock;
use ttf_parser::GlyphId;
use ttf_parser::math::{GlyphAssembly, GlyphConstruction, Table};

/// A bundled math face.
#[derive(Clone, Copy, Debug)]
pub struct FaceInfo {
    /// Human readable name of the face.
    pub label: &'static str,
    /// Family name under which the face is registered with (and requested from) the text system.
    pub family: &'static str,
    /// The bundled font bytes.
    pub data: &'static [u8],
}

/// The fixed face table. A glyph's `font` index is an index into this table.
pub static FACE_TABLE: [FaceInfo; 6] = [
    FaceInfo {
        label: "Noto Sans Math",
        family: "Noto Sans Math",
        data: include_bytes!("fonts/NotoSansMath-Regular.ttf"),
    },
    FaceInfo {
        label: "STIX Two Math",
        family: "STIX Two Math",
        data: include_bytes!("fonts/STIXTwoMath-Regular.otf"),
    },
    FaceInfo {
        label: "Latin Modern Math",
        family: "Latin Modern Math",
        data: include_bytes!("fonts/LatinModernMath-Regular.otf"),
    },
    FaceInfo {
        label: "Libertinus Math",
        family: "Libertinus Math",
        data: include_bytes!("fonts/LibertinusMath-Regular.otf"),
    },
    FaceInfo {
        label: "TeX Gyre Pagella Math",
        family: "TeX Gyre Pagella Math",
        data: include_bytes!("fonts/TeXGyrePagellaMath-Regular.otf"),
    },
    FaceInfo {
        label: "Euler Math",
        family: "Euler Math",
        data: include_bytes!("fonts/EulerMath-Regular.otf"),
    },
];

/// Number of faces in [`FACE_TABLE`].
pub const FACE_COUNT: usize = FACE_TABLE.len();

/// The face whose no-break-space advance defines the reserved slot unit of inline formulas
/// (CMP-012 R4): one fixed reference face regardless of the selected math face.
pub const SLOT_REFERENCE_FACE: usize = 0;

/// The face that backs glyphs the selected face lacks: the Times-style serif that covers every
/// glyph the engine uses.
pub const BACKING_FACE: usize = 1;

/// A selectable math face: an index into [`FACE_TABLE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MathFace(u8);

impl MathFace {
    /// The default sans face, Noto Sans Math.
    pub const NOTO_SANS_MATH: MathFace = MathFace(0);
    /// The Times-style serif, STIX Two Math; it also backs glyphs missing from the other faces.
    pub const STIX_TWO_MATH: MathFace = MathFace(1);
    /// The classic-LaTeX face, Latin Modern Math.
    pub const LATIN_MODERN_MATH: MathFace = MathFace(2);
    /// The warm old-style book serif, Libertinus Math.
    pub const LIBERTINUS_MATH: MathFace = MathFace(3);
    /// The calligraphic-humanist book serif, TeX Gyre Pagella Math.
    pub const TEX_GYRE_PAGELLA_MATH: MathFace = MathFace(4);
    /// The upright handwritten-style face, Euler Math.
    pub const EULER_MATH: MathFace = MathFace(5);
    /// The default face: a sans face that sits beside UI sans-serifs.
    pub const DEFAULT: MathFace = Self::NOTO_SANS_MATH;

    /// The face's human-readable label.
    pub fn label(self) -> &'static str {
        self.info().label
    }

    /// The face at `index` of the face table, if there is one.
    pub fn from_index(index: usize) -> Option<MathFace> {
        (index < FACE_COUNT).then_some(MathFace(index as u8))
    }

    /// Every selectable face in table order.
    pub fn all() -> impl Iterator<Item = MathFace> {
        (0..FACE_COUNT).map(|index| MathFace(index as u8))
    }

    /// The index of this face in [`FACE_TABLE`].
    pub fn index(self) -> usize {
        usize::from(self.0)
    }

    /// The table row of this face.
    pub fn info(self) -> &'static FaceInfo {
        &FACE_TABLE[self.index()]
    }
}

impl Default for MathFace {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// MATH constants of a face in ems (design units divided by units per em).
#[derive(Clone, Copy, Debug, Default)]
pub struct Constants {
    /// Smallest overlap of connected assembly parts.
    pub min_connector_overlap: f32,
    pub radical_degree_bottom_raise: f32,
    pub radical_kern_after_degree: f32,
    pub radical_kern_before_degree: f32,
    pub radical_extra_ascender: f32,
    pub radical_rule_thickness: f32,
    pub radical_display_style_vertical_gap: f32,
    pub radical_vertical_gap: f32,
    pub underbar_extra_descender: f32,
    pub underbar_rule_thickness: f32,
    pub underbar_vertical_gap: f32,
    pub overbar_extra_ascender: f32,
    pub overbar_rule_thickness: f32,
    pub overbar_vertical_gap: f32,
    pub fraction_denom_display_style_gap_min: f32,
    pub fraction_denominator_gap_min: f32,
    pub fraction_rule_thickness: f32,
    pub fraction_num_display_style_gap_min: f32,
    pub fraction_numerator_gap_min: f32,
    pub fraction_denominator_display_style_shift_down: f32,
    pub fraction_denominator_shift_down: f32,
    pub fraction_numerator_display_style_shift_up: f32,
    pub fraction_numerator_shift_up: f32,
    pub stack_display_style_gap_min: f32,
    pub stack_gap_min: f32,
    pub stack_bottom_display_style_shift_down: f32,
    pub stack_bottom_shift_down: f32,
    pub stack_top_display_style_shift_up: f32,
    pub stack_top_shift_up: f32,
    pub lower_limit_baseline_drop_min: f32,
    pub lower_limit_gap_min: f32,
    pub upper_limit_baseline_rise_min: f32,
    pub upper_limit_gap_min: f32,
    pub space_after_script: f32,
    pub superscript_bottom_max_with_subscript: f32,
    pub sub_superscript_gap_min: f32,
    pub superscript_baseline_drop_max: f32,
    pub superscript_bottom_min: f32,
    pub superscript_shift_up_cramped: f32,
    pub superscript_shift_up: f32,
    pub subscript_baseline_drop_min: f32,
    pub subscript_top_max: f32,
    pub subscript_shift_down: f32,
    pub flattened_accent_base_height: f32,
    pub accent_base_height: f32,
    pub axis_height: f32,
    pub delimited_sub_formula_min_height: f32,
    pub display_operator_min_height: f32,
    pub script_script_percent: f32,
    pub script_percent: f32,
}

/// Ink box of a glyph in ems (y up).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlyphBox {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

/// One size variant of a stretchy glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Variant {
    pub glyph: GlyphId,
    /// Size along the stretch axis in ems.
    pub measure: f32,
}

/// One part of a glyph assembly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub glyph: GlyphId,
    pub start_connector: f32,
    pub end_connector: f32,
    pub full_advance: f32,
    pub extender: bool,
}

/// A parsed face with the MATH data of the engine.
pub struct FaceData {
    index: usize,
    face: ttf_parser::Face<'static>,
    math: Table<'static>,
    upem: f32,
    constants: Constants,
}

static PARSED: [OnceLock<Option<FaceData>>; FACE_COUNT] = [const { OnceLock::new() }; FACE_COUNT];

/// The parsed face at `index`, parsing it on first use. `None` when the bundled data does not
/// parse or has no MATH table (never the case for the bundled faces; a unit test pins it).
pub fn face_data(index: usize) -> Option<&'static FaceData> {
    PARSED
        .get(index)?
        .get_or_init(|| FaceData::parse(index))
        .as_ref()
}

/// Whether the face at `index` has been parsed (a face nobody uses is never parsed).
pub fn is_parsed(index: usize) -> bool {
    PARSED.get(index).is_some_and(|cell| cell.get().is_some())
}

fn value(v: ttf_parser::math::MathValue<'_>, upem: f32) -> f32 {
    f32::from(v.value) / upem
}

impl FaceData {
    fn parse(index: usize) -> Option<FaceData> {
        let info = FACE_TABLE.get(index)?;
        let face = ttf_parser::Face::parse(info.data, 0).ok()?;
        let math = face.tables().math?;
        let upem = f32::from(face.units_per_em());
        let c = math.constants?;
        let v = |m| value(m, upem);
        let constants = Constants {
            script_percent: f32::from(c.script_percent_scale_down()) / 100.0,
            script_script_percent: f32::from(c.script_script_percent_scale_down()) / 100.0,
            display_operator_min_height: f32::from(c.display_operator_min_height()) / upem,
            delimited_sub_formula_min_height: f32::from(c.delimited_sub_formula_min_height())
                / upem,
            axis_height: v(c.axis_height()),
            accent_base_height: v(c.accent_base_height()),
            flattened_accent_base_height: v(c.flattened_accent_base_height()),
            subscript_shift_down: v(c.subscript_shift_down()),
            subscript_top_max: v(c.subscript_top_max()),
            subscript_baseline_drop_min: v(c.subscript_baseline_drop_min()),
            superscript_shift_up: v(c.superscript_shift_up()),
            superscript_shift_up_cramped: v(c.superscript_shift_up_cramped()),
            superscript_bottom_min: v(c.superscript_bottom_min()),
            superscript_baseline_drop_max: v(c.superscript_baseline_drop_max()),
            sub_superscript_gap_min: v(c.sub_superscript_gap_min()),
            superscript_bottom_max_with_subscript: v(c.superscript_bottom_max_with_subscript()),
            space_after_script: v(c.space_after_script()),
            upper_limit_gap_min: v(c.upper_limit_gap_min()),
            upper_limit_baseline_rise_min: v(c.upper_limit_baseline_rise_min()),
            lower_limit_gap_min: v(c.lower_limit_gap_min()),
            lower_limit_baseline_drop_min: v(c.lower_limit_baseline_drop_min()),
            stack_top_shift_up: v(c.stack_top_shift_up()),
            stack_top_display_style_shift_up: v(c.stack_top_display_style_shift_up()),
            stack_bottom_shift_down: v(c.stack_bottom_shift_down()),
            stack_bottom_display_style_shift_down: v(c.stack_bottom_display_style_shift_down()),
            stack_gap_min: v(c.stack_gap_min()),
            stack_display_style_gap_min: v(c.stack_display_style_gap_min()),
            fraction_numerator_shift_up: v(c.fraction_numerator_shift_up()),
            fraction_numerator_display_style_shift_up: v(
                c.fraction_numerator_display_style_shift_up()
            ),
            fraction_denominator_shift_down: v(c.fraction_denominator_shift_down()),
            fraction_denominator_display_style_shift_down: v(
                c.fraction_denominator_display_style_shift_down()
            ),
            fraction_numerator_gap_min: v(c.fraction_numerator_gap_min()),
            fraction_num_display_style_gap_min: v(c.fraction_num_display_style_gap_min()),
            fraction_rule_thickness: v(c.fraction_rule_thickness()),
            fraction_denominator_gap_min: v(c.fraction_denominator_gap_min()),
            fraction_denom_display_style_gap_min: v(c.fraction_denom_display_style_gap_min()),
            overbar_vertical_gap: v(c.overbar_vertical_gap()),
            overbar_rule_thickness: v(c.overbar_rule_thickness()),
            overbar_extra_ascender: v(c.overbar_extra_ascender()),
            underbar_vertical_gap: v(c.underbar_vertical_gap()),
            underbar_rule_thickness: v(c.underbar_rule_thickness()),
            underbar_extra_descender: v(c.underbar_extra_descender()),
            radical_vertical_gap: v(c.radical_vertical_gap()),
            radical_display_style_vertical_gap: v(c.radical_display_style_vertical_gap()),
            radical_rule_thickness: v(c.radical_rule_thickness()),
            radical_extra_ascender: v(c.radical_extra_ascender()),
            radical_kern_before_degree: v(c.radical_kern_before_degree()),
            radical_kern_after_degree: v(c.radical_kern_after_degree()),
            radical_degree_bottom_raise: f32::from(c.radical_degree_bottom_raise_percent()) / 100.0,
            min_connector_overlap: math.variants.map_or(0.0, |variants| {
                f32::from(variants.min_connector_overlap) / upem
            }),
        };
        Some(FaceData {
            index,
            face,
            math,
            upem,
            constants,
        })
    }

    /// Index of this face in [`FACE_TABLE`].
    pub fn index(&self) -> usize {
        self.index
    }

    /// MATH constants in ems.
    pub fn constants(&self) -> &Constants {
        &self.constants
    }

    /// Units per em of the face.
    pub fn units_per_em(&self) -> f32 {
        self.upem
    }

    /// The glyph of `ch`, if the face maps it to a real glyph.
    pub fn glyph(&self, ch: char) -> Option<GlyphId> {
        self.face.glyph_index(ch).filter(|glyph| glyph.0 != 0)
    }

    /// Horizontal advance of `glyph` in ems.
    pub fn advance(&self, glyph: GlyphId) -> f32 {
        self.face
            .glyph_hor_advance(glyph)
            .map_or(0.0, |advance| f32::from(advance) / self.upem)
    }

    /// Ink box of `glyph` in ems; an empty box for glyphs without an outline.
    pub fn bounds(&self, glyph: GlyphId) -> GlyphBox {
        self.face
            .glyph_bounding_box(glyph)
            .map_or(GlyphBox::default(), |rect| GlyphBox {
                x_min: f32::from(rect.x_min) / self.upem,
                y_min: f32::from(rect.y_min) / self.upem,
                x_max: f32::from(rect.x_max) / self.upem,
                y_max: f32::from(rect.y_max) / self.upem,
            })
    }

    /// The italic correction of `glyph` in ems (0 when the face has none).
    pub fn italic_correction(&self, glyph: GlyphId) -> f32 {
        self.math
            .glyph_info
            .and_then(|info| info.italic_corrections)
            .and_then(|table| table.get(glyph))
            .map_or(0.0, |v| value(v, self.upem))
    }

    /// The horizontal position of the top accent attachment point of `glyph` in ems.
    pub fn top_accent_attachment(&self, glyph: GlyphId) -> Option<f32> {
        self.math
            .glyph_info
            .and_then(|info| info.top_accent_attachments)
            .and_then(|table| table.get(glyph))
            .map(|v| value(v, self.upem))
    }

    fn construction(&self, glyph: GlyphId, vertical: bool) -> Option<GlyphConstruction<'static>> {
        let variants = self.math.variants?;
        let table = if vertical {
            variants.vertical_constructions
        } else {
            variants.horizontal_constructions
        };
        table.get(glyph)
    }

    /// The size variants of `glyph` along the stretch axis, smallest first (the glyph itself is
    /// not included unless the face lists it).
    pub fn variants(&self, glyph: GlyphId, vertical: bool) -> Vec<Variant> {
        self.construction(glyph, vertical)
            .map(|construction| {
                construction
                    .variants
                    .into_iter()
                    .map(|variant| Variant {
                        glyph: variant.variant_glyph,
                        measure: f32::from(variant.advance_measurement) / self.upem,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The parts of the assembly of `glyph`, bottom-to-top or left-to-right.
    pub fn assembly(&self, glyph: GlyphId, vertical: bool) -> Option<Vec<Part>> {
        let construction = self.construction(glyph, vertical)?;
        let assembly: GlyphAssembly<'_> = construction.assembly?;
        let parts: Vec<Part> = assembly
            .parts
            .into_iter()
            .map(|part| Part {
                glyph: part.glyph_id,
                start_connector: f32::from(part.start_connector_length) / self.upem,
                end_connector: f32::from(part.end_connector_length) / self.upem,
                full_advance: f32::from(part.full_advance) / self.upem,
                extender: part.part_flags.extender(),
            })
            .collect();
        (!parts.is_empty()).then_some(parts)
    }

    /// The outline of `glyph` fed to `builder` in font units (y up), if it has one.
    pub fn outline(&self, glyph: GlyphId, builder: &mut dyn ttf_parser::OutlineBuilder) -> bool {
        self.face.outline_glyph(glyph, builder).is_some()
    }

    /// The glyph an unmodified text system must return for `ch` when `family` is requested.
    pub fn reference_glyph(&self, ch: char) -> Option<u16> {
        self.glyph(ch).map(|glyph| glyph.0)
    }

    /// The face's name table family name.
    pub fn family_name(&self) -> Option<String> {
        self.face
            .names()
            .into_iter()
            .filter(|name| name.name_id == ttf_parser::name_id::FAMILY)
            .find_map(|name| name.to_string())
    }
}

/// The face lookup order for a selected face: the selected face first, then the backing face
/// ([`BACKING_FACE`]), then every other face in table order (a glyph missing from the selected
/// face is backed by the others). Lazy: a face is parsed only when the search reaches it.
pub fn lookup_order(selected: MathFace) -> impl Iterator<Item = usize> {
    std::iter::once(selected.index())
        .chain((selected.index() != BACKING_FACE).then_some(BACKING_FACE))
        .chain((0..FACE_COUNT).filter(move |&i| i != selected.index() && i != BACKING_FACE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_face_parses_with_a_complete_math_table() {
        for face in MathFace::all() {
            let name = face.label();
            let data = face_data(face.index()).expect("face parses");
            let c = data.constants();
            for (what, value) in [
                ("script_percent", c.script_percent),
                ("script_script_percent", c.script_script_percent),
                ("display_operator_min_height", c.display_operator_min_height),
                (
                    "delimited_sub_formula_min_height",
                    c.delimited_sub_formula_min_height,
                ),
                ("axis_height", c.axis_height),
                ("subscript_shift_down", c.subscript_shift_down),
                ("subscript_top_max", c.subscript_top_max),
                ("superscript_shift_up", c.superscript_shift_up),
                (
                    "superscript_shift_up_cramped",
                    c.superscript_shift_up_cramped,
                ),
                ("superscript_bottom_min", c.superscript_bottom_min),
                ("sub_superscript_gap_min", c.sub_superscript_gap_min),
                ("upper_limit_gap_min", c.upper_limit_gap_min),
                ("lower_limit_gap_min", c.lower_limit_gap_min),
                ("stack_top_shift_up", c.stack_top_shift_up),
                ("stack_bottom_shift_down", c.stack_bottom_shift_down),
                ("stack_gap_min", c.stack_gap_min),
                ("fraction_numerator_shift_up", c.fraction_numerator_shift_up),
                (
                    "fraction_denominator_shift_down",
                    c.fraction_denominator_shift_down,
                ),
                ("fraction_numerator_gap_min", c.fraction_numerator_gap_min),
                ("fraction_rule_thickness", c.fraction_rule_thickness),
                (
                    "fraction_denominator_gap_min",
                    c.fraction_denominator_gap_min,
                ),
                ("overbar_vertical_gap", c.overbar_vertical_gap),
                ("overbar_rule_thickness", c.overbar_rule_thickness),
                ("underbar_vertical_gap", c.underbar_vertical_gap),
                ("underbar_rule_thickness", c.underbar_rule_thickness),
                ("radical_vertical_gap", c.radical_vertical_gap),
                (
                    "radical_display_style_vertical_gap",
                    c.radical_display_style_vertical_gap,
                ),
                ("radical_rule_thickness", c.radical_rule_thickness),
            ] {
                assert!(
                    value.is_finite() && value > 0.0,
                    "{name}: MATH constant {what} is {value}"
                );
            }
            assert!(
                c.script_percent < 1.0 && c.script_script_percent < c.script_percent,
                "{name}: script scales"
            );
            assert!(c.min_connector_overlap > 0.0, "{name}: connector overlap");
            assert_eq!(
                data.family_name().as_deref(),
                Some(face.info().family),
                "{name}: the registered family name is the font's own family name"
            );
        }
    }

    #[test]
    fn every_bundled_face_has_stretchy_delimiter_data_and_the_alphabets() {
        for face in MathFace::all() {
            let name = face.label();
            let data = face_data(face.index()).expect("face parses");
            for ch in [
                '(', ')', '[', ']', '{', '}', '\u{221A}', '\u{222B}', '\u{2211}',
            ] {
                let glyph = data
                    .glyph(ch)
                    .unwrap_or_else(|| panic!("{name}: no glyph for {ch:?}"));
                assert!(
                    data.variants(glyph, true).len() >= 2,
                    "{name}: {ch:?} has no vertical size variants"
                );
            }
            let brace = data.glyph('{').unwrap();
            let parts = data
                .assembly(brace, true)
                .unwrap_or_else(|| panic!("{name}: no vertical assembly for a brace"));
            assert!(parts.iter().any(|part| part.extender), "{name}");
            for ch in [
                'A', 'a', 'x', '0', '+', '=', '\u{3B1}', '\u{3C0}', '\u{2192}', '\u{2264}',
            ] {
                let glyph = data
                    .glyph(ch)
                    .unwrap_or_else(|| panic!("{name}: no glyph for {ch:?}"));
                assert!(data.advance(glyph) > 0.0, "{name}: {ch:?}");
            }
            for ch in ['\u{1D44E}', '\u{1D49C}', '\u{211D}', '\u{1D53B}'] {
                assert!(
                    data.glyph(ch).is_some(),
                    "{name}: no mathematical alphabet glyph for {ch:?}"
                );
            }
        }
    }

    #[test]
    fn the_lookup_order_starts_with_the_selected_face_then_the_backing_face() {
        for face in MathFace::all() {
            let order: Vec<usize> = lookup_order(face).collect();
            assert_eq!(order.len(), FACE_COUNT, "{}", face.label());
            assert_eq!(order[0], face.index());
            if face.index() != BACKING_FACE {
                assert_eq!(order[1], BACKING_FACE, "{}", face.label());
            }
            let mut sorted = order.clone();
            sorted.sort_unstable();
            assert!(
                sorted.iter().copied().eq(0..FACE_COUNT),
                "every face exactly once"
            );
        }
        assert_eq!(FACE_TABLE[BACKING_FACE].family, "STIX Two Math");
    }

    #[test]
    fn the_reference_slot_face_has_a_no_break_space() {
        let data = face_data(SLOT_REFERENCE_FACE).unwrap();
        let glyph = data.glyph('\u{a0}').expect("no-break space");
        let advance = data.advance(glyph);
        assert!(advance > 0.2 && advance < 0.3, "{advance}");
    }
}
