//! Proof that a font shapes a set of glyphs pair by pair, for
//! [`PlatformTextSystem::shapes_pairwise`](vespertine::PlatformTextSystem::shapes_pairwise).
//!
//! The shaper maps every character to its nominal glyph and advance, then applies the GSUB and
//! GPOS lookups of the enabled features. A lookup only acts at a glyph its coverage holds, and a
//! contextual lookup only where its whole context can be formed. So in a line made only of the
//! glyphs of a set `G`, when no enabled GSUB lookup and no enabled GPOS lookup other than pair
//! adjustment can act on a sequence of `G`'s glyphs, the glyphs stay the nominal ones and each
//! advance changes only by the adjustment of its pair with the next glyph. Pair adjustments
//! that also move the second glyph are excluded: after applying one, the shaper skips that
//! glyph as the first glyph of the next pair, a context of three glyphs. Anything the proof does
//! not model (AAT tables, legacy kerning, feature variations, unreadable tables) fails closed.

use cosmic_text::skrifa::{
    FontRef, GlyphId, MetadataProvider,
    raw::{
        ReadError, TableProvider,
        collections::IntSet,
        tables::{
            gpos::PositionSubtables,
            layout::{FeatureList, Intersect, LookupFlag},
        },
        types::Tag,
    },
};

/// Features that the shaper does not enable for left-to-right horizontal text unless they are
/// requested. Character variants (`cv01`–`cv99`) and stylistic sets (`ss01`–`ss20`) are off too.
/// Every other feature a font defines counts as enabled. `frac`, `numr` and `dnom` apply only
/// around U+2044, which the callers never pass.
const OFF_BY_DEFAULT: &[&[u8; 4]] = &[
    b"aalt", b"afrc", b"c2pc", b"c2sc", b"case", b"cpct", b"cpsp", b"cswh", b"dlig", b"dnom",
    b"expt", b"falt", b"frac", b"fwid", b"halt", b"hist", b"hkna", b"hlig", b"hngl", b"hojo",
    b"hwid", b"ital", b"jalt", b"jp04", b"jp78", b"jp83", b"jp90", b"lnum", b"mgrk", b"nalt",
    b"nlck", b"numr", b"onum", b"ordn", b"ornm", b"palt", b"pcap", b"pkna", b"pnum", b"pwid",
    b"qwid", b"rtla", b"rtlm", b"ruby", b"salt", b"sinf", b"size", b"smcp", b"smpl", b"subs",
    b"sups", b"swsh", b"titl", b"tnum", b"trad", b"twid", b"unic", b"valt", b"vert", b"vhal",
    b"vkna", b"vkrn", b"vpal", b"vrt2", b"vrtr", b"zero",
];

/// Tables whose shaping the proof does not model.
const UNMODELED_TABLES: &[&[u8; 4]] = &[b"morx", b"mort", b"kerx", b"kern", b"trak"];

/// Whether every line made only of `characters` shapes pair by pair in the face `index` of
/// `data`, with the features `requested` (tag and value) in addition to the default ones.
pub(crate) fn shapes_pairwise(
    data: &[u8],
    index: u32,
    characters: impl IntoIterator<Item = char>,
    requested: &[([u8; 4], u32)],
) -> bool {
    prove(data, index, characters, requested).unwrap_or(false)
}

fn prove(
    data: &[u8],
    index: u32,
    characters: impl IntoIterator<Item = char>,
    requested: &[([u8; 4], u32)],
) -> Result<bool, ReadError> {
    let font = FontRef::from_index(data, index)?;
    if UNMODELED_TABLES
        .iter()
        .any(|tag| font.table_data(Tag::new(tag)).is_some())
    {
        return Ok(false);
    }
    let charmap = font.charmap();
    let mut glyphs = IntSet::<GlyphId>::empty();
    for character in characters {
        match charmap.map(character) {
            Some(glyph) if glyph != GlyphId::NOTDEF => glyphs.insert(glyph),
            _ => return Ok(false),
        };
    }
    // Lookups that skip a class of glyphs would pair glyphs that are not neighbours; base
    // glyphs are skipped only by lookups that say so, which pair adjustment is checked for.
    if let Ok(gdef) = font.gdef()
        && let Some(classes) = gdef.glyph_class_def()
    {
        let classes = classes?;
        if glyphs.iter().any(|glyph| classes.get(glyph) > 1) {
            return Ok(false);
        }
    }
    let enabled = |tag: Tag| {
        let bytes = tag.to_be_bytes();
        if let Some((_, value)) = requested.iter().find(|(requested, _)| *requested == bytes) {
            return *value != 0 || !is_off_by_default(&bytes);
        }
        !is_off_by_default(&bytes)
    };

    if let Ok(gsub) = font.gsub() {
        if gsub.feature_variations().is_some() {
            return Ok(false);
        }
        let lookups = gsub.lookup_list()?.lookups();
        for index in enabled_lookups(&gsub.feature_list()?, enabled)? {
            if lookups.get(index)?.intersects(&glyphs)? {
                return Ok(false);
            }
        }
    }
    if let Ok(gpos) = font.gpos() {
        if gpos.feature_variations().is_some() {
            return Ok(false);
        }
        let lookups = gpos.lookup_list()?.lookups();
        for index in enabled_lookups(&gpos.feature_list()?, enabled)? {
            let lookup = lookups.get(index)?;
            match lookup.subtables()? {
                PositionSubtables::Pair(subtables) => {
                    for subtable in subtables.iter() {
                        let subtable = subtable?;
                        if subtable.intersects(&glyphs)?
                            && (lookup
                                .lookup_flag()
                                .contains(LookupFlag::IGNORE_BASE_GLYPHS)
                                || subtable.value_format2().bits() != 0)
                        {
                            return Ok(false);
                        }
                    }
                }
                other => {
                    if other.intersects(&glyphs)? {
                        return Ok(false);
                    }
                }
            }
        }
    }
    Ok(true)
}

fn is_off_by_default(tag: &[u8; 4]) -> bool {
    let numbered = |prefix: &[u8; 2]| {
        tag[..2] == prefix[..] && tag[2].is_ascii_digit() && tag[3].is_ascii_digit()
    };
    OFF_BY_DEFAULT.contains(&tag) || numbered(b"cv") || numbered(b"ss")
}

/// Indices of the lookups of every enabled feature, in any script and language.
fn enabled_lookups(
    features: &FeatureList<'_>,
    enabled: impl Fn(Tag) -> bool,
) -> Result<Vec<usize>, ReadError> {
    let mut indices = Vec::new();
    for record in features.feature_records() {
        if !enabled(record.feature_tag()) {
            continue;
        }
        let feature = record.feature(features.offset_data())?;
        indices.extend(
            feature
                .lookup_list_indices()
                .iter()
                .map(|index| usize::from(index.get())),
        );
    }
    indices.sort_unstable();
    indices.dedup();
    Ok(indices)
}
