//! TeX-style math layout driven by the OpenType MATH table (CMP-012 R1, R3).
//!
//! The algorithm is the TeX math typesetting model of Appendix G of the TeXbook with the font
//! parameters replaced by the MATH constants, glyph variants and assemblies of the face. Every
//! length is in ems of the surrounding text; a style's size factor (script, script-script) comes
//! from the face's `ScriptPercentScaleDown` constants. Everything here is deterministic and
//! window independent.

use super::faces::{self, Constants, FaceData, MathFace, Part};
use super::formula::{DisplayItem, Formula};
use super::parse::{
    self, Align, Atom, Class, Frac, Group, Kind, Level, Limits, MathError, Matrix, Node,
};
use ttf_parser::GlyphId;

/// Source limit in bytes.
pub const MAX_SOURCE_BYTES: usize = 16 * 1024;
/// Item limit independent of the source length.
pub const MAX_ITEMS: usize = 20_000;
/// Widest accepted advance in ems.
pub const MAX_WIDTH_EM: f32 = 2000.0;
/// Tallest accepted ascent plus descent in ems.
pub const MAX_HEIGHT_EM: f32 = 200.0;

/// The item limit for a source of `len` bytes: min(20000, 64 + 8 x len).
pub fn item_limit(len: usize) -> usize {
    MAX_ITEMS.min(64 + 8 * len)
}

/// Lays `source` out: the engine entry point. Run it on a thread with a few MiB of stack (the
/// formula cache does).
pub fn layout(source: &str, display: bool, face: MathFace) -> Result<Formula, MathError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(MathError::TooLong);
    }
    let tree = parse::parse(source)?;
    let order: Vec<&'static FaceData> = faces::lookup_order(face)
        .filter_map(faces::face_data)
        .collect();
    let primary = *order.first().ok_or(MathError::MissingGlyph('?'))?;
    let mut ctx = Ctx {
        order,
        c: *primary.constants(),
        budget: item_limit(source.len()),
    };
    let style = Style {
        level: if display { Level::Display } else { Level::Text },
        cramped: false,
    };
    let list = ctx.list(&tree, style)?;
    if !(list.w.is_finite() && list.h.is_finite() && list.d.is_finite()) {
        return Err(MathError::NonFinite);
    }
    if list.w > MAX_WIDTH_EM {
        return Err(MathError::TooWide);
    }
    if list.h + list.d > MAX_HEIGHT_EM {
        return Err(MathError::TooTall);
    }
    Formula::from_items(list.items, list.w, list.h, list.d)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Style {
    level: Level,
    cramped: bool,
}

impl Style {
    fn is_display(self) -> bool {
        self.level == Level::Display
    }

    fn is_script(self) -> bool {
        matches!(self.level, Level::Script | Level::ScriptScript)
    }

    fn cram(self) -> Style {
        Style {
            cramped: true,
            ..self
        }
    }

    fn script(self) -> Style {
        Style {
            level: match self.level {
                Level::Display | Level::Text => Level::Script,
                Level::Script | Level::ScriptScript => Level::ScriptScript,
            },
            cramped: self.cramped,
        }
    }

    fn sub(self) -> Style {
        self.script().cram()
    }

    fn frac_num(self) -> Style {
        Style {
            level: match self.level {
                Level::Display => Level::Text,
                Level::Text => Level::Script,
                Level::Script | Level::ScriptScript => Level::ScriptScript,
            },
            cramped: self.cramped,
        }
    }

    fn frac_den(self) -> Style {
        self.frac_num().cram()
    }
}

/// A laid-out box: dimensions above/below the baseline and its items relative to the origin.
#[derive(Clone, Debug, Default)]
struct LBox {
    w: f32,
    h: f32,
    d: f32,
    /// Italic correction of the last glyph.
    ic: f32,
    /// Top accent attachment of a single-glyph box.
    attach: Option<f32>,
    items: Vec<DisplayItem>,
}

impl LBox {
    fn empty() -> LBox {
        LBox::default()
    }

    /// Appends `other` after this box on the baseline.
    fn push_h(&mut self, other: LBox) {
        let x = self.w;
        self.merge(other, x, 0.0);
    }

    /// Merges `other` at horizontal offset `dx` and `up` above the baseline.
    fn merge(&mut self, other: LBox, dx: f32, up: f32) {
        self.h = self.h.max(other.h + up);
        self.d = self.d.max(other.d - up);
        self.w = self.w.max(dx + other.w);
        self.ic = other.ic;
        self.items.reserve(other.items.len());
        for item in other.items {
            self.items.push(shift(item, dx, up));
        }
    }

    /// Moves the whole box `up` above the baseline.
    fn raise(mut self, up: f32) -> LBox {
        self.h += up;
        self.d -= up;
        for item in &mut self.items {
            *item = shift(*item, 0.0, up);
        }
        self
    }

    fn rule(&mut self, x: f32, bottom_up: f32, width: f32, height: f32) {
        self.items.push(DisplayItem::Rule {
            x,
            y: -(bottom_up + height),
            width,
            height,
        });
        self.h = self.h.max(bottom_up + height);
        self.d = self.d.max(-bottom_up);
        self.w = self.w.max(x + width);
    }
}

fn shift(item: DisplayItem, dx: f32, up: f32) -> DisplayItem {
    match item {
        DisplayItem::Glyph {
            font,
            glyph,
            x,
            y,
            scale,
        } => DisplayItem::Glyph {
            font,
            glyph,
            x: x + dx,
            y: y - up,
            scale,
        },
        DisplayItem::Rule {
            x,
            y,
            width,
            height,
        } => DisplayItem::Rule {
            x: x + dx,
            y: y - up,
            width,
            height,
        },
    }
}

struct Ctx {
    /// Faces in lookup order; the first is the selected face.
    order: Vec<&'static FaceData>,
    /// The constants of the selected face.
    c: Constants,
    /// Items that may still be emitted.
    budget: usize,
}

/// Inter-atom spacing codes: `0` none, `1` thin, `2` medium, `3` thick; letters are the same
/// spaces that apply in display and text styles only; `*` cannot occur.
const SPACING: [[u8; 8]; 8] = [
    // Ord Op Bin Rel Open Close Punct Inner
    *b"01bc000a", // Ord
    *b"11*c000a", // Op
    *b"bb**b**b", // Bin
    *b"cc*0c00c", // Rel
    *b"00*00000", // Open
    *b"01bc000a", // Close
    *b"aa*aaaaa", // Punct
    *b"a1bca0aa", // Inner
];

fn class_index(class: Class) -> usize {
    match class {
        Class::Ord => 0,
        Class::Op => 1,
        Class::Bin => 2,
        Class::Rel => 3,
        Class::Open => 4,
        Class::Close => 5,
        Class::Punct => 6,
        Class::Inner => 7,
    }
}

/// The spacing between two classes in mu (18 mu = 1 em).
fn spacing_mu(left: Class, right: Class, script: bool) -> f32 {
    match SPACING[class_index(left)][class_index(right)] {
        b'1' => 3.0,
        b'2' => 4.0,
        b'3' => 5.0,
        b'a' if !script => 3.0,
        b'b' if !script => 4.0,
        b'c' if !script => 5.0,
        _ => 0.0,
    }
}

/// Effective classes after the binary-operator rules of TeX (rule 5 and 6).
fn effective_classes(group: &Group) -> Vec<Option<Class>> {
    let mut classes: Vec<Option<Class>> = group
        .iter()
        .map(|node| match node {
            Node::Atom(atom) => Some(atom.class),
            _ => None,
        })
        .collect();
    let mut prev: Option<usize> = None;
    for ix in 0..classes.len() {
        let Some(class) = classes[ix] else { continue };
        match class {
            Class::Bin => {
                let previous_class = prev.and_then(|p| classes[p]);
                if matches!(
                    previous_class,
                    None | Some(Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct)
                ) {
                    classes[ix] = Some(Class::Ord);
                }
            }
            Class::Rel | Class::Close | Class::Punct => {
                if let Some(p) = prev
                    && classes[p] == Some(Class::Bin)
                {
                    classes[p] = Some(Class::Ord);
                }
            }
            _ => {}
        }
        prev = Some(ix);
    }
    if let Some(p) = prev
        && classes[p] == Some(Class::Bin)
    {
        classes[p] = Some(Class::Ord);
    }
    classes
}

impl Ctx {
    fn scale(&self, style: Style) -> f32 {
        match style.level {
            Level::Display | Level::Text => 1.0,
            Level::Script => self.c.script_percent.max(0.1),
            Level::ScriptScript => self.c.script_script_percent.max(0.1),
        }
    }

    fn take_budget(&mut self, count: usize) -> Result<(), MathError> {
        self.budget = self
            .budget
            .checked_sub(count)
            .ok_or(MathError::TooManyItems)?;
        Ok(())
    }

    fn find_glyph(&self, ch: char) -> Result<(&'static FaceData, GlyphId), MathError> {
        self.order
            .iter()
            .find_map(|face| face.glyph(ch).map(|glyph| (*face, glyph)))
            .ok_or(MathError::MissingGlyph(ch))
    }

    /// A box with one glyph at `scale`.
    fn glyph_box(
        &mut self,
        face: &'static FaceData,
        glyph: GlyphId,
        scale: f32,
    ) -> Result<LBox, MathError> {
        self.take_budget(1)?;
        let bounds = face.bounds(glyph);
        Ok(LBox {
            w: face.advance(glyph) * scale,
            h: bounds.y_max.max(0.0) * scale,
            d: (-bounds.y_min).max(0.0) * scale,
            ic: face.italic_correction(glyph) * scale,
            attach: face
                .top_accent_attachment(glyph)
                .map(|attach| attach * scale),
            items: vec![DisplayItem::Glyph {
                font: face.index() as u8,
                glyph: glyph.0,
                x: 0.0,
                y: 0.0,
                scale,
            }],
        })
    }

    fn char_box(&mut self, ch: char, scale: f32) -> Result<LBox, MathError> {
        let (face, glyph) = self.find_glyph(ch)?;
        self.glyph_box(face, glyph, scale)
    }

    fn text_box(&mut self, text: &str, scale: f32) -> Result<LBox, MathError> {
        let mut out = LBox::empty();
        for ch in text.chars() {
            if ch == ' ' {
                out.w += scale / 3.0;
                continue;
            }
            let glyph = self.char_box(ch, scale)?;
            out.push_h(glyph);
        }
        out.ic = 0.0;
        Ok(out)
    }

    // ---- lists -------------------------------------------------------------------------------

    fn list(&mut self, group: &Group, style: Style) -> Result<LBox, MathError> {
        let classes = effective_classes(group);
        let mut style = style;
        let mut out = LBox::empty();
        let mut previous: Option<Class> = None;
        for (node, class) in group.iter().zip(classes) {
            match node {
                Node::Atom(atom) => {
                    let class = class.unwrap_or(atom.class);
                    let atom_box = self.atom(atom, class, style)?;
                    if let Some(previous) = previous {
                        let mu = spacing_mu(previous, class, style.is_script());
                        out.w += mu / 18.0 * self.scale(style);
                    }
                    out.push_h(atom_box);
                    previous = Some(class);
                }
                Node::Space(em) => out.w += em * self.scale(style),
                Node::Style(level) => style.level = *level,
            }
        }
        Ok(out)
    }

    fn atom(&mut self, atom: &Atom, class: Class, style: Style) -> Result<LBox, MathError> {
        let s = self.scale(style);
        let is_char = matches!(atom.kind, Kind::Char(_));
        // A large operator is a character for the italic correction but a box for the script
        // drops (TeX makes it a box in `make_op`).
        let zero_drops = match atom.kind {
            Kind::Char(ch) => !(class == Class::Op && is_large_operator(ch)),
            _ => false,
        };
        let limits = match atom.limits {
            Limits::Always => true,
            Limits::DisplayOnly => style.is_display(),
            Limits::Never => false,
        } && (atom.sub.is_some() || atom.sup.is_some());
        let mut nucleus = match &atom.kind {
            Kind::Char(ch) => {
                if class == Class::Op && is_large_operator(*ch) {
                    self.large_operator(*ch, style)?
                } else {
                    self.char_box(*ch, s)?
                }
            }
            Kind::Group(group) => self.list(group, style)?,
            Kind::Text(text) => self.text_box(text, s)?,
            Kind::OpName(name) => self.text_box(name, s)?,
            Kind::Frac(frac) => self.fraction(frac, style)?,
            Kind::Sqrt { degree, body } => self.radical(degree.as_ref(), body, style)?,
            Kind::Fenced {
                open,
                close,
                parts,
                middles,
            } => self.fenced(*open, *close, parts, middles, style)?,
            Kind::Sized { size, ch } => self.sized_delimiter(*size, *ch, style)?,
            Kind::Matrix(matrix) => self.matrix(matrix, style)?,
            Kind::Accent {
                ch,
                body,
                over,
                wide,
            } => self.accent(*ch, body, *over, *wide, style)?,
            Kind::Bar { over, body } => self.bar(*over, body, style)?,
            Kind::Stack { base, over, under } => {
                self.stack(base, over.as_ref(), under.as_ref(), style)?
            }
            Kind::Phantom {
                body,
                horizontal,
                vertical,
            } => {
                let inner = self.list(body, style)?;
                LBox {
                    w: if *horizontal { inner.w } else { 0.0 },
                    h: if *vertical { inner.h } else { 0.0 },
                    d: if *vertical { inner.d } else { 0.0 },
                    ..LBox::empty()
                }
            }
        };
        if atom.sub.is_none() && atom.sup.is_none() {
            if is_char {
                // Italic correction of a lone character widens it (TeX rule 17).
                nucleus.w += nucleus.ic;
            }
            return Ok(nucleus);
        }
        if limits {
            self.with_limits(nucleus, atom, style)
        } else {
            self.with_scripts(nucleus, is_char, zero_drops, atom, style)
        }
    }

    // ---- scripts -----------------------------------------------------------------------------

    fn with_scripts(
        &mut self,
        mut nucleus: LBox,
        is_char: bool,
        zero_drops: bool,
        atom: &Atom,
        style: Style,
    ) -> Result<LBox, MathError> {
        let s = self.scale(style);
        let script_scale = self.scale(style.script());
        let sup = match &atom.sup {
            Some(group) => Some(self.list(group, style.script())?),
            None => None,
        };
        let sub = match &atom.sub {
            Some(group) => Some(self.list(group, style.sub())?),
            None => None,
        };
        let c = self.c;
        let mut delta = 0.0;
        if is_char {
            delta = nucleus.ic;
            if sub.is_none() {
                nucleus.w += delta;
                delta = 0.0;
            }
        }
        let (mut u, mut v) = if zero_drops {
            (0.0, 0.0)
        } else {
            (
                nucleus.h - c.superscript_baseline_drop_max * script_scale,
                nucleus.d + c.subscript_baseline_drop_min * script_scale,
            )
        };
        match (&sup, &sub) {
            (Some(sup), None) => {
                let p = if style.cramped {
                    c.superscript_shift_up_cramped
                } else {
                    c.superscript_shift_up
                } * s;
                u = u.max(p).max(sup.d + c.superscript_bottom_min * s);
                v = 0.0;
            }
            (None, Some(sub)) => {
                v = v
                    .max(c.subscript_shift_down * s)
                    .max(sub.h - c.subscript_top_max * s);
                u = 0.0;
            }
            (Some(sup), Some(sub)) => {
                let p = if style.cramped {
                    c.superscript_shift_up_cramped
                } else {
                    c.superscript_shift_up
                } * s;
                u = u.max(p).max(sup.d + c.superscript_bottom_min * s);
                v = v.max(c.subscript_shift_down * s);
                let gap = (u - sup.d) - (sub.h - v);
                if gap < c.sub_superscript_gap_min * s {
                    v += c.sub_superscript_gap_min * s - gap;
                }
                let psi = c.superscript_bottom_max_with_subscript * s - (u - sup.d);
                if psi > 0.0 {
                    u += psi;
                    v -= psi;
                }
            }
            (None, None) => {}
        }
        let base_w = nucleus.w;
        let mut out = nucleus;
        let mut right = base_w;
        if let Some(sup) = sup {
            let w = sup.w;
            out.merge(sup, base_w + delta, u);
            right = right.max(base_w + delta + w);
        }
        if let Some(sub) = sub {
            let w = sub.w;
            out.merge(sub, base_w, -v);
            right = right.max(base_w + w);
        }
        out.w = right + c.space_after_script * s;
        out.ic = 0.0;
        Ok(out)
    }

    fn with_limits(&mut self, nucleus: LBox, atom: &Atom, style: Style) -> Result<LBox, MathError> {
        let top = match &atom.sup {
            Some(group) => Some(self.list(group, style.script())?),
            None => None,
        };
        let bottom = match &atom.sub {
            Some(group) => Some(self.list(group, style.sub())?),
            None => None,
        };
        let delta = nucleus.ic;
        Ok(self.stack_limits(nucleus, top, bottom, delta, style))
    }

    /// Places `top` above and `bottom` below `nucleus`, all centered (rule 13a); the upper box is
    /// shifted right and the lower box left by half the italic correction `delta`.
    fn stack_limits(
        &self,
        nucleus: LBox,
        top: Option<LBox>,
        bottom: Option<LBox>,
        delta: f32,
        style: Style,
    ) -> LBox {
        let s = self.scale(style);
        let c = &self.c;
        let mut width = nucleus.w;
        if let Some(top) = &top {
            width = width.max(top.w + delta.abs());
        }
        if let Some(bottom) = &bottom {
            width = width.max(bottom.w + delta.abs());
        }
        let mut out = LBox::empty();
        out.w = width;
        let nucleus_h = nucleus.h;
        let nucleus_d = nucleus.d;
        let x = (width - nucleus.w) / 2.0;
        out.merge(nucleus, x, 0.0);
        if let Some(top) = top {
            let up = (c.upper_limit_gap_min * s + nucleus_h + top.d)
                .max(c.upper_limit_baseline_rise_min * s);
            let x = (width - top.w) / 2.0 + delta / 2.0;
            out.merge(top, x, up);
        }
        if let Some(bottom) = bottom {
            let down = (c.lower_limit_gap_min * s + nucleus_d + bottom.h)
                .max(c.lower_limit_baseline_drop_min * s);
            let x = (width - bottom.w) / 2.0 - delta / 2.0;
            out.merge(bottom, x, -down);
        }
        out.w = width;
        out.ic = 0.0;
        out
    }

    fn stack(
        &mut self,
        base: &Group,
        over: Option<&Group>,
        under: Option<&Group>,
        style: Style,
    ) -> Result<LBox, MathError> {
        let nucleus = self.list(base, style)?;
        let top = match over {
            Some(group) => Some(self.list(group, style.script())?),
            None => None,
        };
        let bottom = match under {
            Some(group) => Some(self.list(group, style.sub())?),
            None => None,
        };
        Ok(self.stack_limits(nucleus, top, bottom, 0.0, style))
    }

    // ---- operators ---------------------------------------------------------------------------

    fn large_operator(&mut self, ch: char, style: Style) -> Result<LBox, MathError> {
        let s = self.scale(style);
        let (face, base) = self.find_glyph(ch)?;
        let mut glyph = base;
        if style.is_display() {
            let min = self.c.display_operator_min_height * s;
            let variants = face.variants(base, true);
            glyph = variants
                .iter()
                .find(|variant| variant.measure * s >= min)
                .or(variants.last())
                .map_or(base, |variant| variant.glyph);
        }
        let mut glyph_box = self.glyph_box(face, glyph, s)?;
        // Center on the axis.
        let axis = self.c.axis_height * s;
        let down = (glyph_box.h - glyph_box.d) / 2.0 - axis;
        glyph_box = glyph_box.raise(-down);
        Ok(glyph_box)
    }

    // ---- fractions ---------------------------------------------------------------------------

    fn fraction(&mut self, frac: &Frac, style: Style) -> Result<LBox, MathError> {
        let fstyle = Style {
            level: frac.forced.unwrap_or(style.level),
            cramped: style.cramped,
        };
        let s = self.scale(fstyle);
        let c = self.c;
        let num = self.list(&frac.num, fstyle.frac_num())?;
        let den = self.list(&frac.den, fstyle.frac_den())?;
        let display = fstyle.is_display();
        let axis = c.axis_height * s;
        let thickness = if frac.rule {
            c.fraction_rule_thickness * s
        } else {
            0.0
        };
        let (mut up, mut down);
        if frac.rule {
            up = if display {
                c.fraction_numerator_display_style_shift_up
            } else {
                c.fraction_numerator_shift_up
            } * s;
            down = if display {
                c.fraction_denominator_display_style_shift_down
            } else {
                c.fraction_denominator_shift_down
            } * s;
            let num_gap = if display {
                c.fraction_num_display_style_gap_min
            } else {
                c.fraction_numerator_gap_min
            } * s;
            let den_gap = if display {
                c.fraction_denom_display_style_gap_min
            } else {
                c.fraction_denominator_gap_min
            } * s;
            up = up.max(axis + thickness / 2.0 + num_gap + num.d);
            down = down.max(den.h + den_gap + thickness / 2.0 - axis);
        } else {
            up = if display {
                c.stack_top_display_style_shift_up
            } else {
                c.stack_top_shift_up
            } * s;
            down = if display {
                c.stack_bottom_display_style_shift_down
            } else {
                c.stack_bottom_shift_down
            } * s;
            let gap_min = if display {
                c.stack_display_style_gap_min
            } else {
                c.stack_gap_min
            } * s;
            let gap = (up - num.d) - (den.h - down);
            if gap < gap_min {
                let adjust = (gap_min - gap) / 2.0;
                up += adjust;
                down += adjust;
            }
        }
        let width = num.w.max(den.w);
        let mut bar = LBox::empty();
        bar.w = width;
        let (num_w, den_w) = (num.w, den.w);
        bar.merge(num, (width - num_w) / 2.0, up);
        bar.merge(den, (width - den_w) / 2.0, -down);
        if frac.rule {
            bar.rule(0.0, axis - thickness / 2.0, width, thickness);
        }
        bar.w = width;
        bar.ic = 0.0;
        let null_space = 0.12 * s;
        let target = if display {
            c.display_operator_min_height.max(2.39) * s
        } else {
            1.01 * s
        };
        let mut out = LBox::empty();
        match frac.open {
            Some(ch) => {
                let delim = self.delimiter(ch, target, style)?;
                out.push_h(delim);
            }
            None => out.w += null_space,
        }
        out.push_h(bar);
        match frac.close {
            Some(ch) => {
                let delim = self.delimiter(ch, target, style)?;
                out.push_h(delim);
            }
            None => out.w += null_space,
        }
        Ok(out)
    }

    // ---- radicals ----------------------------------------------------------------------------

    fn radical(
        &mut self,
        degree: Option<&Group>,
        body: &Group,
        style: Style,
    ) -> Result<LBox, MathError> {
        let s = self.scale(style);
        let c = self.c;
        let body = self.list(body, style.cram())?;
        let thickness = c.radical_rule_thickness * s;
        let mut gap = if style.is_display() {
            c.radical_display_style_vertical_gap
        } else {
            c.radical_vertical_gap
        } * s;
        let needed = body.h + body.d + gap + thickness;
        let sign = self.stretch('\u{221A}', needed, true, s)?;
        let sign_total = sign.h + sign.d;
        let excess = (sign_total - needed).max(0.0);
        gap += excess / 2.0;
        let rule_top = body.h + gap + thickness;
        let bottom = -(body.d + excess / 2.0);
        // The sign box keeps its own baseline; its bottom goes to `bottom`.
        let sign_up = bottom + sign.d;
        let sign_w = sign.w;
        let body_w = body.w;
        let body_d = body.d;
        let degree = match degree {
            Some(group) => Some(self.list(
                group,
                Style {
                    level: Level::ScriptScript,
                    cramped: false,
                },
            )?),
            None => None,
        };
        let mut x_sign = 0.0;
        let mut degree_place = None;
        if let Some(degree) = degree {
            let kern_before = c.radical_kern_before_degree * s;
            let kern_after = c.radical_kern_after_degree * s;
            x_sign = (kern_before + degree.w + kern_after).max(0.0);
            let x_degree = x_sign - degree.w - kern_after;
            let radical_height = rule_top + body_d + excess / 2.0;
            let up = bottom + c.radical_degree_bottom_raise * radical_height + degree.d;
            degree_place = Some((degree, x_degree, up));
        }
        let mut out = LBox::empty();
        let mut pad = 0.0f32;
        if let Some((_, x_degree, _)) = &degree_place
            && *x_degree < 0.0
        {
            pad = -*x_degree;
        }
        out.merge(sign, x_sign + pad, sign_up);
        out.merge(body, x_sign + pad + sign_w, 0.0);
        out.rule(
            x_sign + pad + sign_w,
            rule_top - thickness,
            body_w,
            thickness,
        );
        out.h = out.h.max(rule_top + c.radical_extra_ascender * s);
        if let Some((degree, x_degree, up)) = degree_place {
            out.merge(degree, x_degree + pad, up);
        }
        out.ic = 0.0;
        out.attach = None;
        Ok(out)
    }

    // ---- stretchy glyphs ---------------------------------------------------------------------

    /// A stretchy glyph at least `target` ems long (in the units of `scale`d ems), vertical or
    /// horizontal. The box sits on the baseline for horizontal glyphs; vertical glyphs keep
    /// their own baseline (a single glyph) or have their bottom on the baseline (an assembly).
    fn stretch(
        &mut self,
        ch: char,
        target: f32,
        vertical: bool,
        scale: f32,
    ) -> Result<LBox, MathError> {
        let (face, base) = self.find_glyph(ch)?;
        let base_bounds = face.bounds(base);
        let base_measure = if vertical {
            base_bounds.y_max - base_bounds.y_min
        } else {
            face.advance(base)
        };
        let mut candidates = vec![faces::Variant {
            glyph: base,
            measure: base_measure,
        }];
        candidates.extend(face.variants(base, vertical));
        if let Some(variant) = candidates
            .iter()
            .find(|variant| variant.measure * scale >= target)
        {
            return self.glyph_box(face, variant.glyph, scale);
        }
        if let Some(parts) = face.assembly(base, vertical) {
            return self.assemble(face, &parts, target, vertical, scale);
        }
        let largest = candidates.last().map_or(base, |variant| variant.glyph);
        self.glyph_box(face, largest, scale)
    }

    fn assemble(
        &mut self,
        face: &'static FaceData,
        parts: &[Part],
        target: f32,
        vertical: bool,
        scale: f32,
    ) -> Result<LBox, MathError> {
        let overlap_min = face.constants().min_connector_overlap;
        let extenders = parts.iter().filter(|part| part.extender).count();
        let fixed_advance: f32 = parts
            .iter()
            .filter(|part| !part.extender)
            .map(|part| part.full_advance)
            .sum();
        let extender_advance: f32 = parts
            .iter()
            .filter(|part| part.extender)
            .map(|part| part.full_advance)
            .sum();
        let fixed_count = parts.len() - extenders;
        let max_repeats = if extenders == 0 { 1 } else { 256 };
        let mut repeats = if extenders == 0 { 1 } else { 1 };
        loop {
            let count = fixed_count + repeats * extenders;
            let total = fixed_advance + repeats as f32 * extender_advance;
            let size = total - count.saturating_sub(1) as f32 * overlap_min;
            if size * scale >= target || repeats >= max_repeats || extenders == 0 {
                break;
            }
            repeats += 1;
        }
        let mut sequence: Vec<Part> = Vec::new();
        for part in parts {
            let copies = if part.extender { repeats } else { 1 };
            for _ in 0..copies {
                sequence.push(*part);
            }
        }
        let total: f32 = sequence.iter().map(|part| part.full_advance).sum();
        let junctions = sequence.len().saturating_sub(1);
        let mut overlap_max = f32::MAX;
        for pair in sequence.windows(2) {
            overlap_max = overlap_max.min(pair[0].end_connector.min(pair[1].start_connector));
        }
        let overlap = if junctions == 0 {
            0.0
        } else {
            ((total - target / scale) / junctions as f32)
                .clamp(overlap_min, overlap_max.max(overlap_min))
        };
        self.take_budget(sequence.len())?;
        let mut out = LBox::empty();
        let mut position = 0.0f32;
        for part in &sequence {
            let bounds = face.bounds(part.glyph);
            let (x, up) = if vertical {
                (0.0, position - bounds.y_min * scale)
            } else {
                (position * scale, 0.0)
            };
            let glyph_box = LBox {
                w: face.advance(part.glyph) * scale,
                h: bounds.y_max.max(0.0) * scale,
                d: (-bounds.y_min).max(0.0) * scale,
                ic: 0.0,
                attach: None,
                items: vec![DisplayItem::Glyph {
                    font: face.index() as u8,
                    glyph: part.glyph.0,
                    x: 0.0,
                    y: 0.0,
                    scale,
                }],
            };
            if vertical {
                out.merge(glyph_box, x, up);
            } else {
                let mut glyph_box = glyph_box;
                glyph_box.h = glyph_box.h.max(0.0);
                out.merge(glyph_box, x, 0.0);
            }
            position += (part.full_advance - overlap) * scale;
        }
        if vertical {
            out.h = out.h.max(position + overlap * scale);
            out.d = 0.0;
        } else {
            out.w = out.w.max(position + overlap * scale);
        }
        Ok(out)
    }

    /// A vertically stretched delimiter at least `target` tall, centered on the axis.
    fn delimiter(&mut self, ch: char, target: f32, style: Style) -> Result<LBox, MathError> {
        let s = self.scale(style);
        let delim = self.stretch(ch, target, true, s)?;
        let axis = self.c.axis_height * s;
        let down = (delim.h - delim.d) / 2.0 - axis;
        let mut delim = delim.raise(-down);
        delim.ic = 0.0;
        Ok(delim)
    }

    fn sized_delimiter(
        &mut self,
        size: u8,
        ch: Option<char>,
        style: Style,
    ) -> Result<LBox, MathError> {
        let Some(ch) = ch else {
            return Ok(LBox::empty());
        };
        let s = self.scale(style);
        let target = match size {
            1 => 1.2,
            2 => 1.8,
            3 => 2.4,
            _ => 3.0,
        } * s;
        self.delimiter(ch, target, style)
    }

    fn delimiter_target(&self, inner: &LBox, style: Style) -> f32 {
        let s = self.scale(style);
        let axis = self.c.axis_height * s;
        let delta = (inner.h - axis).max(inner.d + axis);
        (delta * 2.0 * 0.901).max(delta * 2.0 - 0.5 * s)
    }

    fn fence(
        &mut self,
        inner: LBox,
        open: Option<char>,
        close: Option<char>,
        style: Style,
    ) -> Result<LBox, MathError> {
        let target = self.delimiter_target(&inner, style);
        let mut out = LBox::empty();
        if let Some(ch) = open {
            let delim = self.delimiter(ch, target, style)?;
            out.push_h(delim);
        }
        out.push_h(inner);
        if let Some(ch) = close {
            let delim = self.delimiter(ch, target, style)?;
            out.push_h(delim);
        }
        out.ic = 0.0;
        Ok(out)
    }

    fn fenced(
        &mut self,
        open: Option<char>,
        close: Option<char>,
        parts: &[Group],
        middles: &[Option<char>],
        style: Style,
    ) -> Result<LBox, MathError> {
        let mut boxes = Vec::with_capacity(parts.len());
        let (mut h, mut d) = (0.0f32, 0.0f32);
        for part in parts {
            let part_box = self.list(part, style)?;
            h = h.max(part_box.h);
            d = d.max(part_box.d);
            boxes.push(part_box);
        }
        let reference = LBox {
            h,
            d,
            ..LBox::empty()
        };
        let target = self.delimiter_target(&reference, style);
        let mut out = LBox::empty();
        if let Some(ch) = open {
            let delim = self.delimiter(ch, target, style)?;
            out.push_h(delim);
        }
        let mut boxes = boxes.into_iter();
        if let Some(first) = boxes.next() {
            out.push_h(first);
        }
        for (middle, part_box) in middles.iter().zip(boxes) {
            if let Some(ch) = middle {
                let delim = self.delimiter(*ch, target, style)?;
                out.push_h(delim);
            }
            out.push_h(part_box);
        }
        if let Some(ch) = close {
            let delim = self.delimiter(ch, target, style)?;
            out.push_h(delim);
        }
        out.ic = 0.0;
        Ok(out)
    }

    // ---- matrices ----------------------------------------------------------------------------

    fn matrix(&mut self, matrix: &Matrix, style: Style) -> Result<LBox, MathError> {
        let cell_style = Style {
            level: matrix.level,
            cramped: false,
        };
        let sc = self.scale(cell_style);
        let columns = matrix.aligns.len();
        let mut cells: Vec<Vec<LBox>> = Vec::with_capacity(matrix.rows.len());
        let mut widths = vec![0.0f32; columns];
        let mut heights = Vec::with_capacity(matrix.rows.len());
        let mut depths = Vec::with_capacity(matrix.rows.len());
        for row in &matrix.rows {
            let mut boxes = Vec::with_capacity(row.len());
            let (mut h, mut d) = (0.84 * sc, 0.36 * sc);
            for (ix, cell) in row.iter().enumerate() {
                let cell_box = self.list(cell, cell_style)?;
                widths[ix] = widths[ix].max(cell_box.w);
                h = h.max(cell_box.h);
                d = d.max(cell_box.d);
                boxes.push(cell_box);
            }
            heights.push(h);
            depths.push(d);
            cells.push(boxes);
        }
        // Row baselines below the first one.
        let mut baselines = Vec::with_capacity(cells.len());
        let mut y = 0.0f32;
        for ix in 0..cells.len() {
            if ix > 0 {
                y += (1.2 * sc).max(depths[ix - 1] + heights[ix] + 0.1 * sc);
            }
            baselines.push(y);
        }
        let total_top = heights.first().copied().unwrap_or(0.0);
        let total_bottom =
            baselines.last().copied().unwrap_or(0.0) + depths.last().copied().unwrap_or(0.0);
        let axis = self.c.axis_height * self.scale(style);
        let center = (-total_top + total_bottom) / 2.0;
        let dy = -axis - center;
        let mut column_x = Vec::with_capacity(columns);
        let mut x = 0.0f32;
        for (ix, width) in widths.iter().enumerate() {
            column_x.push(x);
            x += width + matrix.gaps.get(ix).copied().unwrap_or(0.0) * sc;
        }
        let total_width = widths.iter().sum::<f32>() + matrix.gaps.iter().sum::<f32>() * sc;
        let mut body = LBox::empty();
        for (row_ix, row) in cells.into_iter().enumerate() {
            for (col_ix, cell) in row.into_iter().enumerate() {
                let slack = widths[col_ix] - cell.w;
                let offset = match matrix.aligns[col_ix] {
                    Align::Left => 0.0,
                    Align::Center => slack / 2.0,
                    Align::Right => slack,
                };
                // Positions are measured downwards from the first baseline.
                body.merge(cell, column_x[col_ix] + offset, -(baselines[row_ix] + dy));
            }
        }
        body.w = total_width;
        body.h = body.h.max(total_top - dy);
        body.d = body.d.max(total_bottom + dy);
        body.ic = 0.0;
        if matrix.open.is_some() || matrix.close.is_some() {
            self.fence(body, matrix.open, matrix.close, style)
        } else {
            Ok(body)
        }
    }

    // ---- accents and bars --------------------------------------------------------------------

    fn accent(
        &mut self,
        ch: char,
        body: &Group,
        over: bool,
        wide: bool,
        style: Style,
    ) -> Result<LBox, MathError> {
        let s = self.scale(style);
        let c = self.c;
        let body = self.list(body, if over { style.cram() } else { style })?;
        let combining = matches!(ch, '\u{300}'..='\u{36F}' | '\u{20D0}'..='\u{20FF}');
        let mark = if wide {
            self.stretch(ch, body.w, false, s)?
        } else {
            self.char_box(ch, s)?
        };
        let (face, glyph) = self.find_glyph(ch)?;
        let mark_attach = mark.attach.unwrap_or_else(|| {
            if mark.w > 0.0 {
                mark.w / 2.0
            } else {
                let bounds = face.bounds(glyph);
                (bounds.x_min + bounds.x_max) / 2.0 * s
            }
        });
        let body_attach = body.attach.unwrap_or(body.w / 2.0);
        let mut x_mark = body_attach - mark_attach;
        if wide {
            // Stretched marks are centered over the whole body.
            let mark_center = if mark.w > 0.0 {
                mark.w / 2.0
            } else {
                mark_attach
            };
            x_mark = body.w / 2.0 - mark_center;
        }
        let mut pad = 0.0f32;
        if x_mark < 0.0 {
            pad = -x_mark;
        }
        let mut out = LBox::empty();
        let (body_w, body_h, body_d) = (body.w, body.h, body.d);
        let up;
        if over {
            up = if combining {
                (body_h - c.accent_base_height * s).max(0.0)
            } else {
                body_h + c.upper_limit_gap_min * s + mark.d
            };
        } else {
            up = -(body_d + c.underbar_vertical_gap * s + mark.h);
        }
        let attach = body.attach;
        out.merge(body, pad, 0.0);
        out.merge(mark, x_mark + pad, up);
        out.w = out.w.max(body_w + pad);
        out.ic = 0.0;
        out.attach = attach.map(|a| a + pad);
        Ok(out)
    }

    fn bar(&mut self, over: bool, body: &Group, style: Style) -> Result<LBox, MathError> {
        let s = self.scale(style);
        let c = self.c;
        let body = self.list(body, if over { style.cram() } else { style })?;
        let (thickness, gap, extra) = if over {
            (
                c.overbar_rule_thickness * s,
                c.overbar_vertical_gap * s,
                c.overbar_extra_ascender * s,
            )
        } else {
            (
                c.underbar_rule_thickness * s,
                c.underbar_vertical_gap * s,
                c.underbar_extra_descender * s,
            )
        };
        self.take_budget(1)?;
        let (w, h, d) = (body.w, body.h, body.d);
        let mut out = body;
        if over {
            out.rule(0.0, h + gap, w, thickness);
            out.h += extra;
        } else {
            out.rule(0.0, -(d + gap + thickness), w, thickness);
            out.d += extra;
        }
        out.ic = 0.0;
        Ok(out)
    }
}

/// Characters whose nucleus is a large operator (sized up in display style, centered on the
/// axis).
fn is_large_operator(ch: char) -> bool {
    matches!(
        ch,
        '\u{2211}'
            | '\u{220F}'
            | '\u{2210}'
            | '\u{222B}'..='\u{222E}'
            | '\u{22C0}'..='\u{22C3}'
            | '\u{2A00}'
            | '\u{2A01}'
            | '\u{2A02}'
            | '\u{2A04}'
            | '\u{2A06}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(source: &str, display: bool) -> Formula {
        layout(source, display, MathFace::DEFAULT).unwrap_or_else(|e| panic!("{source}: {e}"))
    }

    #[test]
    fn plain_text_is_a_row_of_glyphs_on_the_baseline() {
        let formula = lay("x+y", false);
        assert!(formula.advance() > 1.0);
        assert!(formula.ascent() > 0.0 && formula.descent() >= 0.0);
        assert!(formula.items().len() >= 3);
    }

    #[test]
    fn a_fraction_is_taller_than_its_parts_and_has_a_rule() {
        let frac = lay(r"\frac{a}{b}", false);
        let plain = lay("a", false);
        assert!(frac.height() > plain.height() * 1.5);
        assert!(
            frac.items()
                .iter()
                .any(|item| matches!(item, DisplayItem::Rule { .. }))
        );
    }

    #[test]
    fn display_style_fractions_are_taller_than_text_style() {
        let text = lay(r"\frac{a}{b}", false);
        let display = lay(r"\frac{a}{b}", true);
        assert!(display.height() > text.height());
    }

    #[test]
    fn scripts_shift_by_the_math_constants() {
        let sup = lay("x^2", false);
        let plain = lay("x", false);
        assert!(sup.ascent() > plain.ascent());
        let sub = lay("x_2", false);
        assert!(sub.descent() > plain.descent());
        let both = lay("x_2^2", false);
        assert!(both.height() > sup.height());
    }

    #[test]
    fn big_operators_take_limits_in_display_style_only() {
        let display = lay(r"\sum_{i=0}^{n} i", true);
        let text = lay(r"\sum_{i=0}^{n} i", false);
        assert!(display.height() > text.height());
        assert!(display.advance() < text.advance() + 2.0);
    }

    #[test]
    fn radicals_delimiters_and_matrices_lay_out() {
        for source in [
            r"\sqrt{x^2+1}",
            r"\sqrt[3]{x}",
            r"\left( \frac{a}{b} \right)",
            r"\left\{ \begin{matrix} a & b \\ c & d \end{matrix} \right.",
            r"\begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}",
            r"\begin{cases} x & x>0 \\ -x & x\le 0 \end{cases}",
            r"\int_0^1 f(x)\,dx",
            r"\mathbb{R}, \mathcal{L}, \mathfrak{g}",
            r"\hat{x}, \widehat{xyz}, \vec{v}, \overline{AB}",
            r"\underbrace{a+b}_{n}",
            r"\sum_{k=1}^{n} \binom{n}{k}",
            r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
            r"\text{if } x \in A",
            r"e^{i\pi} + 1 = 0",
        ] {
            let formula = lay(source, true);
            assert!(formula.advance() > 0.0, "{source}");
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let a = lay(r"\frac{\sqrt{a}}{b_1^2}", true);
        let b = lay(r"\frac{\sqrt{a}}{b_1^2}", true);
        assert_eq!(a, b);
    }

    #[test]
    fn engine_limits_are_errors() {
        assert_eq!(
            layout(&"x".repeat(16385), false, MathFace::DEFAULT).unwrap_err(),
            MathError::TooLong
        );
        assert!(layout(&"x".repeat(16384), false, MathFace::DEFAULT).is_ok());
        let wide = r"\qquad ".repeat(1200);
        assert_eq!(
            layout(&wide, false, MathFace::DEFAULT).unwrap_err(),
            MathError::TooWide
        );
        let rows = format!(r"\begin{{matrix}}{}\end{{matrix}}", r"a \\ ".repeat(200));
        assert_eq!(
            layout(&rows, true, MathFace::DEFAULT).unwrap_err(),
            MathError::TooTall
        );
        assert!(matches!(
            layout(r"\unknowncommand", false, MathFace::DEFAULT),
            Err(MathError::Syntax(_))
        ));
    }

    #[test]
    fn the_item_limit_follows_the_source_length() {
        assert_eq!(item_limit(0), 64);
        assert_eq!(item_limit(10), 144);
        assert_eq!(item_limit(100_000), 20_000);
        // Ten glyphs from a three byte source exceed the limit of 64 + 24 only through items:
        let source = "x".repeat(10);
        assert!(layout(&source, false, MathFace::DEFAULT).is_ok());
    }
}
