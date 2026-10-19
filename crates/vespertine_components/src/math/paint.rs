//! Painting formulas over their reserved slots (CMP-012 R8, R11).
//!
//! Glyphs paint through the text system's glyph rasterizer and atlas from math fonts registered
//! on first use. A registered font is acceptable only when it is verified to shape like the
//! bundled bytes: a spread of the face's characters laid out in its family name must return that
//! font with the glyph identifiers the bundled data maps them to. A system copy of a different
//! version (renumbered glyphs) fails verification, and the face's glyphs are then painted as
//! vector outlines from the bundled data (filled non-zero), never through another copy of the
//! family. Rules paint as filled quads.

use super::faces::{self, FACE_TABLE, FaceData, SLOT_REFERENCE_FACE};
use super::formula::{DisplayItem, Formula, MathStyle};
use super::slot::{MathText, Presentation, display_line_height, present_display, space_advance};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use ttf_parser::GlyphId as FontGlyphId;
use vespertine::{
    AnyElement, App, Bounds, ElementId, FillOptions, FillRule, FontId, Global, GlyphId, Hsla,
    InteractiveElement, IntoElement, ParentElement, PathBuilder, PathStyle, Pixels, Point,
    StatefulInteractiveElement, Styled, TextRun, Window, div, fill, font, point, px, size,
};
use vespertine_richtext::{RichLayout, RichText, RunKey};

/// How a face's glyphs are painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceStatus {
    /// The face is registered and verified: glyphs go through the glyph atlas.
    Verified(FontId),
    /// The registered or system font does not shape like the bundled bytes: glyphs are painted
    /// as vector outlines from the bundled data.
    Unverified,
}

/// What a shaper returned for one character of the verification sample: whether the run was
/// set in the requested family's font, the glyph identifier and the character's byte index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShapedSample {
    /// The run's font is the font resolved for the family name.
    pub font_matches: bool,
    /// The glyph identifier the text system produced.
    pub glyph: u32,
    /// Byte index of the character in the sample text.
    pub index: usize,
}

/// The characters of the verification sample: a spread over the face's repertoire.
fn sample_chars(face: &FaceData) -> Vec<char> {
    [
        'A',
        'Z',
        'a',
        'g',
        'x',
        'z',
        '0',
        '5',
        '9',
        '+',
        '=',
        '(',
        ')',
        '[',
        '{',
        '|',
        '\u{1D44E}',
        '\u{1D465}',
        '\u{3B1}',
        '\u{3C0}',
        '\u{393}',
        '\u{2211}',
        '\u{222B}',
        '\u{221A}',
        '\u{2264}',
        '\u{221E}',
        '\u{2202}',
        '\u{211D}',
        '\u{1D49C}',
        '\u{2192}',
        '\u{2212}',
        '\u{22C5}',
    ]
    .into_iter()
    .filter(|ch| face.glyph(*ch).is_some())
    .collect()
}

/// Whether the shaper reproduces the bundled glyph identifiers for the face's sample: `shape`
/// lays the sample text out in the face's family name and reports the glyphs.
pub fn verify_face(index: usize, shape: &mut dyn FnMut(&str) -> Vec<ShapedSample>) -> bool {
    let Some(face) = faces::face_data(index) else {
        return false;
    };
    let chars = sample_chars(face);
    if chars.is_empty() {
        return false;
    }
    let text: String = chars.iter().collect();
    let shaped = shape(&text);
    let mut byte_index = 0;
    for ch in chars {
        let expected = face.reference_glyph(ch).map(u32::from);
        let found = shaped.iter().find(|sample| sample.index == byte_index);
        match (expected, found) {
            (Some(expected), Some(sample)) if sample.font_matches && sample.glyph == expected => {}
            _ => return false,
        }
        byte_index += ch.len_utf8();
    }
    true
}

/// The per-application state of math fonts: which faces are registered and how each paints.
#[derive(Default)]
pub struct MathFonts {
    faces: HashMap<usize, FaceStatus>,
}

impl Global for MathFonts {}

impl MathFonts {
    /// The status of a face, if it has been registered.
    pub fn status(cx: &App, face: usize) -> Option<FaceStatus> {
        cx.try_global::<MathFonts>()
            .and_then(|fonts| fonts.faces.get(&face).copied())
    }

    /// Records the status of a face (used by hosts that verify fonts themselves, and by tests
    /// that simulate a system copy with renumbered glyphs).
    pub fn set_status(cx: &mut App, face: usize, status: FaceStatus) {
        cx.default_global::<MathFonts>().faces.insert(face, status);
    }
}

/// Registers the face with the text system on first use and verifies it; later calls return the
/// recorded status. A face nobody uses is never parsed or registered.
pub fn ensure_face(face: usize, window: &Window, cx: &mut App) -> FaceStatus {
    if let Some(status) = MathFonts::status(cx, face) {
        return status;
    }
    let status = register_and_verify(face, window, cx);
    MathFonts::set_status(cx, face, status);
    status
}

fn register_and_verify(face: usize, window: &Window, cx: &mut App) -> FaceStatus {
    let Some(info) = FACE_TABLE.get(face) else {
        return FaceStatus::Unverified;
    };
    if cx
        .text_system()
        .add_fonts(vec![Cow::Borrowed(info.data)])
        .is_err()
    {
        return FaceStatus::Unverified;
    }
    let family = font(info.family);
    let font_id = cx.text_system().resolve_font(&family);
    let window_text = window.text_system().clone();
    let verified = verify_face(face, &mut |text: &str| {
        let run = TextRun {
            len: text.len(),
            font: family.clone(),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let layout = window_text.layout_line(text, px(16.0), &[run], None);
        layout
            .runs
            .iter()
            .flat_map(|run| {
                run.glyphs.iter().map(move |glyph| ShapedSample {
                    font_matches: run.font_id == font_id,
                    glyph: glyph.id.0,
                    index: glyph.index,
                })
            })
            .collect()
    });
    if verified {
        FaceStatus::Verified(font_id)
    } else {
        FaceStatus::Unverified
    }
}

/// The font of the reserved slot characters: the fixed reference face.
pub fn slot_font() -> vespertine::Font {
    font(FACE_TABLE[SLOT_REFERENCE_FACE].family)
}

/// Where one formula was painted, in logical pixels of the window.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotReport {
    /// Displayed byte range of the slot.
    pub range: Range<usize>,
    /// Left and right edge of the reserved slot.
    pub slot: Range<f32>,
    /// Left and right edge of everything the formula painted.
    pub ink: Range<f32>,
    /// Top and bottom of the formula's box (baseline minus ascent, baseline plus descent).
    pub extent_y: Range<f32>,
    /// Baseline of the painted formula.
    pub baseline: f32,
    /// Top of the line box the slot sits in.
    pub row_top: f32,
    /// Height of the line box.
    pub line_height: f32,
}

/// What a paint did, for hosts and tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaintReport {
    /// Glyphs painted through the glyph atlas.
    pub atlas_glyphs: usize,
    /// Glyphs painted as vector outlines.
    pub outline_glyphs: usize,
    /// Rules painted as filled quads.
    pub rules: usize,
    /// One entry per painted slot.
    pub slots: Vec<SlotReport>,
}

impl PaintReport {
    fn add(&mut self, atlas: usize, outline: usize, rules: usize) {
        self.atlas_glyphs += atlas;
        self.outline_glyphs += outline;
        self.rules += rules;
    }
}

/// Receives the report of every paint of a formula element.
pub type PaintObserver = Rc<dyn Fn(&PaintReport)>;

/// One formula to paint over a displayed slot range with the paint color of the run it replaces.
#[derive(Clone)]
pub struct SlotPaint {
    /// Displayed byte range of the reserved spaces.
    pub range: Range<usize>,
    /// The formula.
    pub formula: Arc<Formula>,
    /// The color of the run under the slot (fade and highlight included).
    pub color: Hsla,
}

/// Paints formulas over the slots of shaped text: inline formulas on the baseline of their line,
/// display formulas centered in their line box (CMP-012 R8).
#[derive(Clone)]
pub struct MathPainter {
    slots: Vec<SlotPaint>,
    scale: f32,
    display: bool,
}

impl MathPainter {
    /// A painter for `slots` at the math `scale` (size / 100).
    pub fn new(slots: Vec<SlotPaint>, scale: f32, display: bool) -> MathPainter {
        MathPainter {
            slots,
            scale,
            display,
        }
    }

    /// The slots this painter paints.
    pub fn slots(&self) -> &[SlotPaint] {
        &self.slots
    }

    /// Paints every formula over its slot using the geometry of the shaped text. Layout is never
    /// repeated: the stored em geometry is scaled by the font size of the text.
    pub fn paint(&self, layout: &RichLayout, window: &mut Window, cx: &mut App) -> PaintReport {
        let mut report = PaintReport::default();
        let line_height = layout.line_height();
        let rows = layout.rows();
        let Some(font_size) = layout
            .text_layout()
            .line_layouts()
            .first()
            .map(|line| line.unwrapped_layout.font_size)
        else {
            return report;
        };
        let unit = f32::from(font_size) * self.scale;
        for slot in &self.slots {
            let (Some(start), Some(end)) = (
                layout.position_for_index(slot.range.start),
                layout.position_for_index(slot.range.end),
            ) else {
                continue;
            };
            let same_row = (end.y - start.y).abs() < px(0.5);
            let slot_width = if same_row {
                f32::from(end.x - start.x)
            } else {
                // A slot wider than the column wraps; the formula starts on the first row.
                slot.range.len() as f32 / 2.0 * space_advance() * f32::from(font_size)
            };
            let extent = slot.formula.extent() * unit;
            let origin_x = f32::from(start.x) + (slot_width - extent) / 2.0
                - slot.formula.extent_left() * unit;
            let baseline = if self.display {
                let height = slot.formula.height() * unit;
                f32::from(start.y)
                    + (f32::from(line_height) - height) / 2.0
                    + slot.formula.ascent() * unit
            } else {
                let row = rows
                    .iter()
                    .rfind(|row| row.top <= start.y + px(0.5))
                    .copied();
                match row {
                    Some(row) => f32::from(start.y) + f32::from(row.baseline_offset(line_height)),
                    None => f32::from(start.y) + f32::from(line_height) * 0.75,
                }
            };
            let (atlas, outline, rules) = paint_formula(
                &slot.formula,
                point(px(origin_x), px(baseline)),
                px(unit),
                slot.color,
                window,
                cx,
            );
            report.add(atlas, outline, rules);
            report.slots.push(SlotReport {
                range: slot.range.clone(),
                slot: f32::from(start.x)..f32::from(start.x) + slot_width,
                ink: origin_x + slot.formula.extent_left() * unit
                    ..origin_x + (slot.formula.extent_left() + slot.formula.extent()) * unit,
                extent_y: baseline - slot.formula.ascent() * unit
                    ..baseline + slot.formula.descent() * unit,
                baseline,
                row_top: f32::from(start.y),
                line_height: f32::from(line_height),
            });
        }
        report
    }
}

/// Paints one formula with its origin (baseline, left) at `origin` and `unit` pixels per em;
/// returns the glyphs painted through the atlas, as outlines, and the rules painted.
pub fn paint_formula(
    formula: &Formula,
    origin: Point<Pixels>,
    unit: Pixels,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> (usize, usize, usize) {
    let (mut atlas, mut outline, mut rules) = (0, 0, 0);
    let unit = f32::from(unit);
    let scale_factor = window.scale_factor();
    for item in formula.items() {
        match *item {
            DisplayItem::Glyph {
                font: face,
                glyph,
                x,
                y,
                scale,
            } => {
                let position = point(origin.x + px(x * unit), origin.y + px(y * unit));
                let size_px = px(scale * unit);
                let status = ensure_face(usize::from(face), window, cx);
                let mut painted = false;
                if let FaceStatus::Verified(font_id) = status {
                    painted = window
                        .paint_glyph(position, font_id, GlyphId(u32::from(glyph)), size_px, color)
                        .is_ok();
                    if painted {
                        atlas += 1;
                    }
                }
                if !painted
                    && paint_outline(usize::from(face), glyph, position, size_px, color, window)
                {
                    outline += 1;
                }
            }
            DisplayItem::Rule {
                x,
                y,
                width,
                height,
            } => {
                let left = f32::from(origin.x) + x * unit;
                let top = f32::from(origin.y) + y * unit;
                // Snap to the device pixel grid so thin bars stay crisp; never thinner than one
                // device pixel.
                let snapped_top = (top * scale_factor).round() / scale_factor;
                let snapped_height =
                    ((height * unit * scale_factor).round() / scale_factor).max(1.0 / scale_factor);
                let bounds = Bounds {
                    origin: point(px(left), px(snapped_top)),
                    size: size(px(width * unit), px(snapped_height)),
                };
                window.paint_quad(fill(bounds, color));
                rules += 1;
            }
        }
    }
    (atlas, outline, rules)
}

/// Paints a glyph as a filled (non-zero) vector outline from the bundled face data.
fn paint_outline(
    face: usize,
    glyph: u16,
    origin: Point<Pixels>,
    size_px: Pixels,
    color: Hsla,
    window: &mut Window,
) -> bool {
    let Some(data) = faces::face_data(face) else {
        return false;
    };
    let scale = f32::from(size_px) / data.units_per_em();
    let mut sink = OutlineSink {
        builder: PathBuilder::fill().with_style(PathStyle::Fill(
            FillOptions::default().with_fill_rule(FillRule::NonZero),
        )),
        origin,
        scale,
        open: false,
    };
    if !data.outline(FontGlyphId(glyph), &mut sink) {
        // A glyph without an outline (a space) paints nothing and is not a failure.
        return false;
    }
    if sink.open {
        sink.builder.close();
    }
    match sink.builder.build() {
        Ok(path) => {
            window.paint_path(path, color);
            true
        }
        Err(_) => false,
    }
}

struct OutlineSink {
    builder: PathBuilder,
    origin: Point<Pixels>,
    scale: f32,
    open: bool,
}

impl OutlineSink {
    fn at(&self, x: f32, y: f32) -> Point<Pixels> {
        point(
            self.origin.x + px(x * self.scale),
            self.origin.y - px(y * self.scale),
        )
    }
}

impl ttf_parser::OutlineBuilder for OutlineSink {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.open {
            self.builder.close();
        }
        let to = self.at(x, y);
        self.builder.move_to(to);
        self.open = true;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let to = self.at(x, y);
        self.builder.line_to(to);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (ctrl, to) = (self.at(x1, y1), self.at(x, y));
        self.builder.curve_to(to, ctrl);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b, to) = (self.at(x1, y1), self.at(x2, y2), self.at(x, y));
        self.builder.cubic_bezier_to(to, a, b);
    }

    fn close(&mut self) {
        if self.open {
            self.builder.close();
            self.open = false;
        }
    }
}

/// The runs of a paragraph with slots: text runs in `text_run`'s style and slot runs in the
/// reference face with the same color and size.
pub fn slot_runs(text: &MathText, text_run: &TextRun) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut offset = 0;
    let push = |runs: &mut Vec<TextRun>, len: usize, font: vespertine::Font| {
        if len > 0 {
            runs.push(TextRun {
                len,
                font,
                ..text_run.clone()
            });
        }
    };
    for slot in text.slots() {
        push(&mut runs, slot.start - offset, text_run.font.clone());
        push(&mut runs, slot.len(), slot_font());
        offset = slot.end;
    }
    push(
        &mut runs,
        text.displayed().len() - offset,
        text_run.font.clone(),
    );
    runs
}

/// Options of [`math_text`].
#[derive(Clone)]
pub struct MathTextOptions {
    /// The math scale (size / 100).
    pub scale: f32,
    /// Whether the text is a display formula (centered in its line box, not on the baseline).
    pub display: bool,
    /// Receives the report of every paint (hosts and tests).
    pub observer: Option<PaintObserver>,
}

impl MathTextOptions {
    /// Options for an inline paragraph at `scale`.
    pub fn inline(scale: f32) -> MathTextOptions {
        MathTextOptions {
            scale,
            display: false,
            observer: None,
        }
    }

    /// Options for a display formula at `scale`.
    pub fn display(scale: f32) -> MathTextOptions {
        MathTextOptions {
            display: true,
            ..Self::inline(scale)
        }
    }

    /// With an observer of paint reports.
    pub fn observer(mut self, observer: PaintObserver) -> MathTextOptions {
        self.observer = Some(observer);
        self
    }
}

/// The element of a paragraph (inline formulas) or display block text: a [`RichText`] over the
/// displayed text with the original-text map, whose paint ends by painting the formulas. Fonts
/// are registered and verified here, before the text is shaped.
pub fn math_text(
    id: impl Into<ElementId>,
    text: &MathText,
    runs: Vec<TextRun>,
    options: MathTextOptions,
    window: &mut Window,
    cx: &mut App,
) -> RichText {
    ensure_face(SLOT_REFERENCE_FACE, window, cx);
    let slots = text
        .slots()
        .iter()
        .zip(text.formulas())
        .map(|(range, formula)| {
            let mut offset = 0;
            let color = runs
                .iter()
                .find(|run| {
                    let covers = offset <= range.start && range.start < offset + run.len;
                    offset += run.len;
                    covers
                })
                .map_or_else(Hsla::default, |run| run.color);
            SlotPaint {
                range: range.clone(),
                formula: formula.clone(),
                color,
            }
        })
        .collect();
    let painter = MathPainter::new(slots, options.scale, options.display);
    let observer = options.observer;
    RichText::with_original(id, text.original_text())
        .runs(runs)
        .after_paint(move |layout, window, cx| {
            let report = painter.paint(layout, window, cx);
            if let Some(observer) = observer {
                observer(&report);
            }
        })
}

/// A display formula as a block, or the fallback the caller renders as a code block.
pub enum MathBlock {
    /// The typeset block: one line of text whose slot shows the formula.
    Typeset(AnyElement),
    /// The formula could not be typeset (math disabled, engine refusal, slot too big): render
    /// `text` as a code block labelled `language` (`math`).
    Fallback {
        /// The language label of the code block.
        language: &'static str,
        /// The source text.
        text: String,
    },
}

/// A display formula (CMP-012 R8, R9): a block as wide as its container whose single line of
/// text holds the reserved slot. The formula is centered horizontally in the block when it
/// fits and the block scrolls horizontally (independently of any outer vertical scroll) when it
/// is wider than the column; it is centered vertically in its line box, whose height is
/// `max(configured, ceil(formula height x scale x text size + 2 x 6))`.
pub struct DisplayBlock<'a> {
    id: ElementId,
    source: &'a str,
    style: MathStyle,
    run: TextRun,
    size: Pixels,
    line_height: Pixels,
    observer: Option<PaintObserver>,
    key: Option<RunKey>,
    native: bool,
}

impl<'a> DisplayBlock<'a> {
    /// A block for `source`. `run` supplies the font and color of the text the block sits in
    /// (the slot characters use the reference face with the same color); `size` is its font
    /// size. The configured line height defaults to 1.5 x size.
    pub fn new(
        id: impl Into<ElementId>,
        source: &'a str,
        style: MathStyle,
        run: TextRun,
        size: Pixels,
    ) -> Self {
        DisplayBlock {
            id: id.into(),
            source,
            style,
            run,
            size,
            line_height: size * 1.5,
            observer: None,
            key: None,
            native: false,
        }
    }

    /// Registers the formula's text with the document selection under `key`, so drag
    /// selections cross the block and copy yields `$$source$$`.
    pub fn key(mut self, key: RunKey) -> Self {
        self.key = Some(key);
        self
    }

    /// Marks the text as generated by a native widget (search washes match the exact string).
    pub fn native(mut self) -> Self {
        self.native = true;
        self
    }

    /// The configured line height of the document's text.
    pub fn line_height(mut self, line_height: Pixels) -> Self {
        self.line_height = line_height;
        self
    }

    /// Receives the report of every paint (hosts and tests).
    pub fn observer(mut self, observer: PaintObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Builds the block, or reports the fallback to render instead.
    pub fn build(self, window: &mut Window, cx: &mut App) -> MathBlock {
        let formula = match present_display(self.source, &self.style) {
            Presentation::Formula { formula, .. } => formula,
            Presentation::CodeBlock { language, text } => {
                return MathBlock::Fallback { language, text };
            }
            Presentation::InlineCode(text) => {
                return MathBlock::Fallback {
                    language: "math",
                    text,
                };
            }
        };
        let scale = self.style.scale();
        let Some(text) = MathText::display(self.source, &formula, scale) else {
            return MathBlock::Fallback {
                language: "math",
                text: self.source.to_string(),
            };
        };
        let line_height = display_line_height(&formula, scale, self.size, self.line_height);
        let runs = slot_runs(&text, &TextRun { len: 0, ..self.run });
        let mut options = MathTextOptions::display(scale);
        options.observer = self.observer;
        let mut rich = math_text((self.id.clone(), "math"), &text, runs, options, window, cx);
        if let Some(key) = self.key {
            rich = rich.key(key);
        }
        if self.native {
            rich = rich.native();
        }
        MathBlock::Typeset(
            div()
                .id((self.id, "scroll"))
                .w_full()
                .flex()
                .overflow_x_scroll()
                .child(
                    div()
                        .mx_auto()
                        .flex_none()
                        .whitespace_nowrap()
                        .line_height(line_height)
                        .text_size(self.size)
                        .child(rich),
                )
                .into_any_element(),
        )
    }
}
