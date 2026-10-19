//! A deterministic synthetic TrueType font builder for tests and examples.
//!
//! The builder emits small but complete `glyf`-flavoured fonts from code, so
//! tests never need copied font assets. The output loads in ttf-parser,
//! fontdb, cosmic-text and swash: tables are sorted, padded and checksummed,
//! `loca` uses the long format and every required `hhea`/`maxp` field is set.
//!
//! ```ignore
//! # use vespertine::test_font::TestFontBuilder;
//! let bytes = TestFontBuilder::new("Registered Sample")
//!     .glyph('A', 1000)
//!     .glyph('B', 2000)
//!     .build();
//! assert!(bytes.len() > 100);
//! ```

use std::collections::BTreeMap;

/// Units per em of every synthetic font.
pub const TEST_FONT_UNITS_PER_EM: u16 = 1000;

const ASCENT: i16 = 800;
const DESCENT: i16 = -200;
const NOTDEF_ADVANCE: u16 = 500;
const SIDE_BEARING: i16 = 50;
const BOX_HEIGHT: i16 = 700;

/// Builds a synthetic TrueType font.
///
/// Glyph advances are expressed in font units; one em is
/// [`TEST_FONT_UNITS_PER_EM`] units.
#[derive(Clone, Debug)]
pub struct TestFontBuilder {
    family: String,
    typographic_family: Option<String>,
    subfamily: String,
    full_name: Option<String>,
    postscript_name: Option<String>,
    include_family_name: bool,
    glyphs: BTreeMap<char, u16>,
    visible_outlines: bool,
    weight: u16,
    italic: bool,
    fixed_pitch_flag: bool,
}

impl TestFontBuilder {
    /// Starts a font declaring `family`, with glyphs for the space (0.25 em),
    /// `A` (1 em), `B` (2 em) and `m` (1 em).
    pub fn new(family: impl Into<String>) -> Self {
        let family = family.into();
        let postscript = family.replace(' ', "");
        let mut glyphs = BTreeMap::new();
        glyphs.insert(' ', 250);
        glyphs.insert('A', 1000);
        glyphs.insert('B', 2000);
        glyphs.insert('m', 1000);
        Self {
            subfamily: "Regular".into(),
            full_name: Some(format!("{family} Regular")),
            postscript_name: Some(format!("{postscript}-Regular")),
            family,
            typographic_family: None,
            include_family_name: true,
            glyphs,
            visible_outlines: true,
            weight: 400,
            italic: false,
            fixed_pitch_flag: false,
        }
    }

    /// Starts a font with every printable ASCII glyph advancing `advance` units.
    pub fn monospace(family: impl Into<String>, advance: u16) -> Self {
        let mut builder = Self::new(family);
        builder.glyphs.clear();
        for ch in ' '..='~' {
            builder.glyphs.insert(ch, advance);
        }
        builder.fixed_pitch_flag = true;
        builder
    }

    /// Starts a font with every printable ASCII glyph, with proportional
    /// advances: `i` 300, `m` 900, `W` 1000, `0` 600, other glyphs 550.
    pub fn proportional(family: impl Into<String>) -> Self {
        let mut builder = Self::new(family);
        builder.glyphs.clear();
        for ch in ' '..='~' {
            let advance = match ch {
                ' ' => 250,
                'i' | 'l' | 'j' | '.' | ',' | ':' | ';' | '!' | '|' | '\'' => 300,
                'm' | 'M' | '@' => 900,
                'W' | 'w' => 1000,
                '0' => 600,
                _ => 550,
            };
            builder.glyphs.insert(ch, advance);
        }
        builder
    }

    /// Adds or replaces the glyph for `ch` with the given advance in font units.
    ///
    /// Only characters in the Basic Multilingual Plane are supported.
    pub fn glyph(mut self, ch: char, advance: u16) -> Self {
        assert!(
            (ch as u32) < 0xFFFF,
            "test fonts only support BMP characters, got {ch:?}"
        );
        self.glyphs.insert(ch, advance);
        self
    }

    /// Adds or replaces a glyph for every character of `chars`.
    pub fn glyphs(mut self, chars: &str, advance: u16) -> Self {
        for ch in chars.chars() {
            self = self.glyph(ch, advance);
        }
        self
    }

    /// Removes the glyph for `ch` so the font does not map it.
    pub fn without_glyph(mut self, ch: char) -> Self {
        self.glyphs.remove(&ch);
        self
    }

    /// Omits the family name record (name id 1).
    pub fn without_family_name(mut self) -> Self {
        self.include_family_name = false;
        self
    }

    /// Omits the full name record (name id 4).
    pub fn without_full_name(mut self) -> Self {
        self.full_name = None;
        self
    }

    /// Omits the PostScript name record (name id 6).
    pub fn without_postscript_name(mut self) -> Self {
        self.postscript_name = None;
        self
    }

    /// Declares a typographic family name (name id 16), which takes precedence
    /// over the family name when addressing the font.
    pub fn typographic_family(mut self, family: impl Into<String>) -> Self {
        self.typographic_family = Some(family.into());
        self
    }

    /// Sets the style name (name id 2), the full name and the PostScript name
    /// suffix, for example `"Italic"`.
    pub fn subfamily(mut self, subfamily: impl Into<String>) -> Self {
        let subfamily = subfamily.into();
        self.full_name = self
            .full_name
            .is_some()
            .then(|| format!("{} {subfamily}", self.family));
        self.postscript_name = self
            .postscript_name
            .is_some()
            .then(|| format!("{}-{subfamily}", self.family.replace(' ', "")));
        self.subfamily = subfamily;
        self
    }

    /// Marks the face italic (OS/2 selection bit, head macStyle, post angle).
    pub fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    /// Sets the OS/2 weight class, for example 700 for bold.
    pub fn weight(mut self, weight: u16) -> Self {
        self.weight = weight;
        self
    }

    /// Sets the `post` table fixed-pitch flag independently of the glyph
    /// advances, to model fonts whose metadata flag is wrong.
    pub fn fixed_pitch_flag(mut self, fixed_pitch: bool) -> Self {
        self.fixed_pitch_flag = fixed_pitch;
        self
    }

    /// Gives every glyph an empty outline instead of a visible box.
    pub fn empty_outlines(mut self) -> Self {
        self.visible_outlines = false;
        self
    }

    /// Serializes the font.
    pub fn build(&self) -> Vec<u8> {
        let chars: Vec<char> = self.glyphs.keys().copied().collect();
        // Glyph 0 is `.notdef`; characters follow in code point order.
        let mut advances = vec![NOTDEF_ADVANCE];
        advances.extend(chars.iter().map(|ch| self.glyphs[ch]));

        let (glyf, loca, lsbs, bounds) = self.glyf_and_loca(&chars, &advances);

        let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"OS/2", self.os2(&chars)),
            (*b"cmap", cmap(&chars)),
            (*b"glyf", glyf),
            (*b"head", self.head(bounds)),
            (*b"hhea", hhea(&advances, &lsbs, bounds)),
            (*b"hmtx", hmtx(&advances, &lsbs)),
            (*b"loca", loca),
            (*b"maxp", maxp(advances.len() as u16, self.visible_outlines)),
            (*b"name", self.name()),
            (*b"post", self.post()),
        ];
        tables.sort_by_key(|(tag, _)| *tag);
        assemble(tables)
    }

    fn glyf_and_loca(
        &self,
        chars: &[char],
        advances: &[u16],
    ) -> (Vec<u8>, Vec<u8>, Vec<i16>, [i16; 4]) {
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        let mut lsbs = Vec::new();
        let mut bounds = [0i16; 4];
        let mut first_box = true;
        for (index, advance) in advances.iter().enumerate() {
            loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
            // `.notdef` and the space stay empty so that blank glyphs are blank.
            let blank = index == 0 || chars[index - 1] == ' ' || !self.visible_outlines;
            if blank || *advance <= (2 * SIDE_BEARING) as u16 {
                lsbs.push(0);
                continue;
            }
            let x_min = SIDE_BEARING;
            let x_max = *advance as i16 - SIDE_BEARING;
            let y_max = BOX_HEIGHT;
            if first_box {
                bounds = [x_min, 0, x_max, y_max];
                first_box = false;
            } else {
                bounds = [
                    bounds[0].min(x_min),
                    bounds[1],
                    bounds[2].max(x_max),
                    bounds[3].max(y_max),
                ];
            }
            lsbs.push(x_min);
            // Simple glyph: one rectangular contour of four on-curve points
            // with 16-bit coordinate deltas.
            push_i16(&mut glyf, 1);
            for value in [x_min, 0, x_max, y_max] {
                push_i16(&mut glyf, value);
            }
            push_u16(&mut glyf, 3); // end point of the only contour
            push_u16(&mut glyf, 0); // no instructions
            glyf.extend_from_slice(&[0x01; 4]); // on-curve, 16-bit deltas
            for delta in [x_min, x_max - x_min, 0, x_min - x_max] {
                push_i16(&mut glyf, delta);
            }
            for delta in [0, 0, y_max, -y_max] {
                push_i16(&mut glyf, delta);
            }
            pad4(&mut glyf);
        }
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        (glyf, loca, lsbs, bounds)
    }

    fn head(&self, bounds: [i16; 4]) -> Vec<u8> {
        let mut t = Vec::with_capacity(54);
        push_u32(&mut t, 0x0001_0000); // version
        push_u32(&mut t, 0x0001_0000); // font revision
        push_u32(&mut t, 0); // checksum adjustment, patched in `assemble`
        push_u32(&mut t, 0x5F0F_3CF5); // magic number
        push_u16(&mut t, 0x0003); // baseline at y=0, lsb at x=0
        push_u16(&mut t, TEST_FONT_UNITS_PER_EM);
        t.extend_from_slice(&0i64.to_be_bytes()); // created
        t.extend_from_slice(&0i64.to_be_bytes()); // modified
        for value in bounds {
            push_i16(&mut t, value);
        }
        push_u16(&mut t, if self.italic { 0x0002 } else { 0 }); // macStyle
        push_u16(&mut t, 8); // lowest recommended ppem
        push_i16(&mut t, 2); // font direction hint
        push_i16(&mut t, 1); // long loca offsets
        push_i16(&mut t, 0); // glyph data format
        t
    }

    fn os2(&self, chars: &[char]) -> Vec<u8> {
        let mut t = Vec::with_capacity(96);
        let average = if self.glyphs.is_empty() {
            0
        } else {
            self.glyphs.values().map(|a| *a as u32).sum::<u32>() / self.glyphs.len() as u32
        };
        push_u16(&mut t, 4); // version
        push_i16(&mut t, average as i16);
        push_u16(&mut t, self.weight);
        push_u16(&mut t, 5); // normal width
        push_u16(&mut t, 0); // embedding permissions
        for value in [650, 600, 0, 75, 650, 600, 0, 350] {
            push_i16(&mut t, value); // subscript and superscript geometry
        }
        push_i16(&mut t, 50); // strikeout size
        push_i16(&mut t, 300); // strikeout position
        push_i16(&mut t, 0); // family class
        let mut panose = [0u8; 10];
        panose[3] = if self.fixed_pitch_flag { 9 } else { 0 };
        t.extend_from_slice(&panose);
        for range in [0x0000_0001u32, 0, 0, 0] {
            push_u32(&mut t, range);
        }
        t.extend_from_slice(b"TEST");
        let mut selection = 0x0080u16; // use typographic metrics
        selection |= match (self.italic, self.weight >= 600) {
            (true, true) => 0x0001 | 0x0020,
            (true, false) => 0x0001,
            (false, true) => 0x0020,
            (false, false) => 0x0040,
        };
        push_u16(&mut t, selection);
        push_u16(&mut t, chars.first().map_or(0xFFFF, |c| *c as u16));
        push_u16(&mut t, chars.last().map_or(0xFFFF, |c| *c as u16));
        push_i16(&mut t, ASCENT);
        push_i16(&mut t, DESCENT);
        push_i16(&mut t, 0); // typographic line gap
        push_u16(&mut t, ASCENT as u16);
        push_u16(&mut t, (-DESCENT) as u16);
        push_u32(&mut t, 1); // code page range 1 (Latin 1)
        push_u32(&mut t, 0);
        push_i16(&mut t, 500); // x height
        push_i16(&mut t, 700); // cap height
        push_u16(&mut t, 0); // default character
        push_u16(&mut t, 0x20); // break character
        push_u16(&mut t, 0); // max context
        t
    }

    fn name(&self) -> Vec<u8> {
        let mut records: Vec<(u16, String)> = Vec::new();
        if self.include_family_name {
            records.push((1, self.family.clone()));
        }
        records.push((2, self.subfamily.clone()));
        records.push((3, format!("{};test", self.family)));
        if let Some(full) = &self.full_name {
            records.push((4, full.clone()));
        }
        records.push((5, "Version 1.000".into()));
        if let Some(postscript) = &self.postscript_name {
            records.push((6, postscript.clone()));
        }
        if let Some(typographic) = &self.typographic_family {
            records.push((16, typographic.clone()));
            records.push((17, self.subfamily.clone()));
        }
        records.sort_by_key(|(id, _)| *id);

        let strings_offset = 6 + 12 * records.len();
        let mut header = Vec::new();
        let mut strings = Vec::new();
        push_u16(&mut header, 0); // format
        push_u16(&mut header, records.len() as u16);
        push_u16(&mut header, strings_offset as u16);
        for (id, text) in &records {
            let encoded: Vec<u8> = text
                .encode_utf16()
                .flat_map(|unit| unit.to_be_bytes())
                .collect();
            push_u16(&mut header, 3); // Windows
            push_u16(&mut header, 1); // Unicode BMP
            push_u16(&mut header, 0x0409); // English (United States)
            push_u16(&mut header, *id);
            push_u16(&mut header, encoded.len() as u16);
            push_u16(&mut header, strings.len() as u16);
            strings.extend_from_slice(&encoded);
        }
        header.extend_from_slice(&strings);
        header
    }

    fn post(&self) -> Vec<u8> {
        let mut t = Vec::with_capacity(32);
        push_u32(&mut t, 0x0003_0000); // format 3: no glyph names
        let angle: i32 = if self.italic { -12 << 16 } else { 0 };
        t.extend_from_slice(&angle.to_be_bytes());
        push_i16(&mut t, -100); // underline position
        push_i16(&mut t, 50); // underline thickness
        push_u32(&mut t, u32::from(self.fixed_pitch_flag));
        for _ in 0..4 {
            push_u32(&mut t, 0); // memory usage hints
        }
        t
    }
}

fn hhea(advances: &[u16], lsbs: &[i16], bounds: [i16; 4]) -> Vec<u8> {
    let mut t = Vec::with_capacity(36);
    let max_advance = advances.iter().copied().max().unwrap_or(0);
    push_u32(&mut t, 0x0001_0000);
    push_i16(&mut t, ASCENT);
    push_i16(&mut t, DESCENT);
    push_i16(&mut t, 0); // line gap
    push_u16(&mut t, max_advance);
    push_i16(&mut t, lsbs.iter().copied().min().unwrap_or(0));
    push_i16(&mut t, 0); // min right side bearing
    push_i16(&mut t, bounds[2]); // max extent
    push_i16(&mut t, 1); // caret slope rise
    push_i16(&mut t, 0); // caret slope run
    push_i16(&mut t, 0); // caret offset
    for _ in 0..4 {
        push_i16(&mut t, 0); // reserved
    }
    push_i16(&mut t, 0); // metric data format
    push_u16(&mut t, advances.len() as u16); // one long metric per glyph
    t
}

fn hmtx(advances: &[u16], lsbs: &[i16]) -> Vec<u8> {
    let mut t = Vec::with_capacity(advances.len() * 4);
    for (advance, lsb) in advances.iter().zip(lsbs) {
        push_u16(&mut t, *advance);
        push_i16(&mut t, *lsb);
    }
    t
}

fn maxp(glyph_count: u16, visible: bool) -> Vec<u8> {
    let mut t = Vec::with_capacity(32);
    push_u32(&mut t, 0x0001_0000);
    push_u16(&mut t, glyph_count);
    let (points, contours) = if visible { (4, 1) } else { (0, 0) };
    for value in [
        points, contours, 0, 0, // max points/contours, composite points/contours
        2, 0, 0, 0, 0, // zones, twilight points, storage, function defs, instruction defs
        0, 0, 0, 0, // stack elements, instruction size, component elements, depth
    ] {
        push_u16(&mut t, value);
    }
    t
}

/// A format 4 cmap with one segment per character, registered for both the
/// Unicode (0, 3) and Windows Unicode BMP (3, 1) encodings.
fn cmap(chars: &[char]) -> Vec<u8> {
    let segment_count = chars.len() + 1;
    let mut subtable = Vec::new();
    push_u16(&mut subtable, 4);
    push_u16(&mut subtable, (16 + segment_count * 8) as u16); // length
    push_u16(&mut subtable, 0); // language
    push_u16(&mut subtable, (segment_count * 2) as u16);
    let entry_selector = (usize::BITS - 1 - segment_count.leading_zeros()) as u16;
    let search_range = 2 * (1u16 << entry_selector);
    push_u16(&mut subtable, search_range);
    push_u16(&mut subtable, entry_selector);
    push_u16(&mut subtable, (segment_count * 2) as u16 - search_range);
    for ch in chars {
        push_u16(&mut subtable, *ch as u16); // end codes
    }
    push_u16(&mut subtable, 0xFFFF);
    push_u16(&mut subtable, 0); // reserved pad
    for ch in chars {
        push_u16(&mut subtable, *ch as u16); // start codes
    }
    push_u16(&mut subtable, 0xFFFF);
    for (index, ch) in chars.iter().enumerate() {
        let glyph = index as u16 + 1;
        push_u16(&mut subtable, glyph.wrapping_sub(*ch as u16)); // id deltas
    }
    push_u16(&mut subtable, 1);
    for _ in 0..segment_count {
        push_u16(&mut subtable, 0); // id range offsets
    }

    let mut t = Vec::new();
    push_u16(&mut t, 0); // version
    push_u16(&mut t, 2); // encoding records
    for (platform, encoding) in [(0u16, 3u16), (3, 1)] {
        push_u16(&mut t, platform);
        push_u16(&mut t, encoding);
        push_u32(&mut t, 20); // subtable offset
    }
    t.extend_from_slice(&subtable);
    t
}

/// Lays out the table directory and padded tables, then patches the head
/// checksum adjustment.
fn assemble(tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    let count = tables.len() as u16;
    let entry_selector = (u16::BITS - 1 - count.leading_zeros()) as u16;
    let search_range = (1u16 << entry_selector) * 16;

    let mut font = Vec::new();
    push_u32(&mut font, 0x0001_0000);
    push_u16(&mut font, count);
    push_u16(&mut font, search_range);
    push_u16(&mut font, entry_selector);
    push_u16(&mut font, count * 16 - search_range);

    let mut offset = 12 + 16 * tables.len();
    let mut head_offset = 0;
    for (tag, data) in &tables {
        font.extend_from_slice(tag);
        push_u32(&mut font, checksum(data));
        push_u32(&mut font, offset as u32);
        push_u32(&mut font, data.len() as u32);
        if tag == b"head" {
            head_offset = offset;
        }
        offset += data.len().next_multiple_of(4);
    }
    for (_, data) in &tables {
        font.extend_from_slice(data);
        pad4(&mut font);
    }
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&font));
    font[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    font
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

fn pad4(buffer: &mut Vec<u8>) {
    while !buffer.len().is_multiple_of(4) {
        buffer.push(0);
    }
}

fn push_u16(buffer: &mut Vec<u8>, value: u16) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

fn push_i16(buffer: &mut Vec<u8>, value: i16) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ttf_parser::{Face, GlyphId};

    fn glyph_advance(face: &Face<'_>, ch: char) -> Option<u16> {
        face.glyph_index(ch)
            .and_then(|glyph| face.glyph_hor_advance(glyph))
    }

    #[test]
    fn default_font_parses_with_requested_advances() {
        let bytes = TestFontBuilder::new("Sample Family").build();
        let face = Face::parse(&bytes, 0).expect("synthetic font parses");
        assert_eq!(face.units_per_em(), TEST_FONT_UNITS_PER_EM);
        assert_eq!(glyph_advance(&face, 'A'), Some(1000));
        assert_eq!(glyph_advance(&face, 'B'), Some(2000));
        assert_eq!(glyph_advance(&face, 'm'), Some(1000));
        assert_eq!(glyph_advance(&face, 'z'), None);
        assert_eq!(face.ascender(), ASCENT);
        assert_eq!(face.descender(), DESCENT);
    }

    #[test]
    fn names_are_present_and_omittable() {
        let name = |face: &Face<'_>, id: u16| {
            face.names()
                .into_iter()
                .find(|name| name.name_id == id)
                .and_then(|name| name.to_string())
        };
        let bytes = TestFontBuilder::new("Sample Family").build();
        let face = Face::parse(&bytes, 0).unwrap();
        assert_eq!(name(&face, 1).as_deref(), Some("Sample Family"));
        assert_eq!(name(&face, 4).as_deref(), Some("Sample Family Regular"));
        assert_eq!(name(&face, 6).as_deref(), Some("SampleFamily-Regular"));

        let bytes = TestFontBuilder::new("X")
            .without_full_name()
            .without_postscript_name()
            .without_family_name()
            .typographic_family("Typographic")
            .build();
        let face = Face::parse(&bytes, 0).unwrap();
        assert_eq!(name(&face, 1), None);
        assert_eq!(name(&face, 4), None);
        assert_eq!(name(&face, 6), None);
        assert_eq!(name(&face, 16).as_deref(), Some("Typographic"));
    }

    #[test]
    fn glyph_ids_follow_code_point_order_and_can_be_removed() {
        let bytes = TestFontBuilder::new("Sample")
            .glyph('a', 400)
            .without_glyph('m')
            .build();
        let face = Face::parse(&bytes, 0).unwrap();
        assert_eq!(face.glyph_index(' '), Some(GlyphId(1)));
        assert_eq!(face.glyph_index('A'), Some(GlyphId(2)));
        assert_eq!(face.glyph_index('B'), Some(GlyphId(3)));
        assert_eq!(face.glyph_index('a'), Some(GlyphId(4)));
        assert_eq!(face.glyph_index('m'), None);
        assert_eq!(face.number_of_glyphs(), 5);
    }

    #[test]
    fn style_metadata_is_reflected() {
        let bytes = TestFontBuilder::monospace("Mono", 600)
            .italic(true)
            .weight(700)
            .subfamily("Bold Italic")
            .build();
        let face = Face::parse(&bytes, 0).unwrap();
        assert!(face.is_italic());
        assert!(face.is_bold());
        assert!(face.is_monospaced());
        assert_eq!(face.weight().to_number(), 700);

        let bytes = TestFontBuilder::proportional("Prop").build();
        let face = Face::parse(&bytes, 0).unwrap();
        assert!(!face.is_italic());
        assert!(!face.is_monospaced());
        assert_eq!(glyph_advance(&face, 'i'), Some(300));
        assert_eq!(glyph_advance(&face, 'W'), Some(1000));
    }

    #[test]
    fn fixed_pitch_flag_is_independent_of_advances() {
        let bytes = TestFontBuilder::proportional("Liar")
            .fixed_pitch_flag(true)
            .build();
        let face = Face::parse(&bytes, 0).unwrap();
        assert!(face.is_monospaced());
        assert_ne!(glyph_advance(&face, 'i'), glyph_advance(&face, 'W'));
    }

    #[test]
    fn outlines_are_visible_or_empty() {
        let visible = TestFontBuilder::new("Boxes").build();
        let face = Face::parse(&visible, 0).unwrap();
        let glyph = face.glyph_index('A').unwrap();
        let bounds = face
            .glyph_bounding_box(glyph)
            .expect("A has a visible outline");
        assert_eq!((bounds.x_min, bounds.x_max), (50, 950));
        assert_eq!((bounds.y_min, bounds.y_max), (0, 700));
        let space = face.glyph_index(' ').unwrap();
        assert!(face.glyph_bounding_box(space).is_none());

        let empty = TestFontBuilder::new("Blank").empty_outlines().build();
        let face = Face::parse(&empty, 0).unwrap();
        let glyph = face.glyph_index('A').unwrap();
        assert!(face.glyph_bounding_box(glyph).is_none());
        assert_eq!(face.glyph_hor_advance(glyph), Some(1000));
    }

    #[test]
    fn table_checksums_are_valid() {
        let bytes = TestFontBuilder::proportional("Checked").build();
        assert_eq!(checksum(&bytes), 0xB1B0_AFBA);
        let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        for record in 0..count {
            let entry = &bytes[12 + record * 16..28 + record * 16];
            let tag = &entry[..4];
            let stored = u32::from_be_bytes(entry[4..8].try_into().unwrap());
            let offset = u32::from_be_bytes(entry[8..12].try_into().unwrap()) as usize;
            let length = u32::from_be_bytes(entry[12..16].try_into().unwrap()) as usize;
            assert_eq!(offset % 4, 0, "table {tag:?} is aligned");
            let mut data = bytes[offset..offset + length].to_vec();
            if tag == b"head" {
                data[8..12].fill(0);
            }
            assert_eq!(stored, checksum(&data), "checksum of {tag:?}");
        }
    }
}
