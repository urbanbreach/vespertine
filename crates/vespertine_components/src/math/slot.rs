//! Reserved formula slots in flowed text, line heights and the fallback presentation
//! (CMP-012 R4, R5, R6, R7, R9).
//!
//! An inline formula reserves horizontal space with a run of no-break spaces set in one fixed
//! reference face. The flattened text of a paragraph therefore holds the slot characters, and a
//! [`DisplayMap`] records for each slot the original delimited source: selection and copy see
//! `$source$` (or `$$source$$` for a display formula), never the placeholders.

use super::faces::{SLOT_REFERENCE_FACE, face_data};
use super::formula::{Formula, MathStyle};
use std::ops::Range;
use std::sync::{Arc, OnceLock};
use vespertine::{Pixels, px};
use vespertine_richtext::{DisplayMap, FlatText, OriginalText, Replacement};

/// The reserved-space character: a no-break space, so a slot is a word character and splits
/// across rows only when wider than the column.
pub const RESERVED_SPACE: char = '\u{a0}';
/// Advance of the reference no-break space when the face cannot tell (em).
pub const DEFAULT_SPACE_ADVANCE: f32 = 0.25;
/// Largest reserved slot width in ems of the surrounding text.
pub const MAX_SLOT_EM: f32 = 8192.0;
/// Tallest inline formula in ems of the surrounding text (ascent plus descent, scaled).
pub const MAX_INLINE_HEIGHT_EM: f32 = 12.0;
/// Vertical pad above and below a display formula, in text-size units.
pub const DISPLAY_PAD: f32 = 6.0;

/// The no-break-space advance of the reference face in ems (read from the face, else 0.25).
pub fn space_advance() -> f32 {
    static ADVANCE: OnceLock<f32> = OnceLock::new();
    *ADVANCE.get_or_init(|| {
        face_data(SLOT_REFERENCE_FACE)
            .and_then(|face| face.glyph(RESERVED_SPACE).map(|glyph| face.advance(glyph)))
            .filter(|advance| advance.is_finite() && *advance > 0.0)
            .unwrap_or(DEFAULT_SPACE_ADVANCE)
    })
}

/// Number of reserved spaces for a painted extent of `extent_em` formula ems at `scale`:
/// ceil(extent x scale / space advance), at least 1; `None` when the width is non-finite,
/// negative or beyond 8192 em.
pub fn slot_chars(extent_em: f32, scale: f32) -> Option<usize> {
    let width = extent_em * scale;
    if !width.is_finite() || !(0.0..=MAX_SLOT_EM).contains(&width) {
        return None;
    }
    Some(((width / space_advance()).ceil() as usize).max(1))
}

/// The reserved slot text for a painted extent (see [`slot_chars`]).
pub fn slot_text_for_extent(extent_em: f32, scale: f32) -> Option<String> {
    slot_chars(extent_em, scale).map(|count| RESERVED_SPACE.to_string().repeat(count))
}

/// The reserved slot text for `formula` at `scale`; `None` when the formula is too big to
/// reserve.
pub fn reserved_slot_text(formula: &Formula, scale: f32) -> Option<String> {
    slot_text_for_extent(formula.extent(), scale)
}

/// Line height of a paragraph with display formulas only: `max(configured, ceil(formula height
/// x scale x size + 2 x 6))`; the pad is in text-size units and does not grow with the math size.
pub fn display_line_height(
    formula: &Formula,
    scale: f32,
    size: Pixels,
    configured: Pixels,
) -> Pixels {
    let height = formula.height() * scale * f32::from(size) + 2.0 * DISPLAY_PAD;
    configured.max(px(height.ceil()))
}

/// The line height of a paragraph with inline formulas (CMP-012 R7).
///
/// `flat` is the flattened paragraph text with its slot ranges, `formulas` the formula of each
/// slot (parallel to `flat.slots`), `metrics` gives the font ascent and descent of a run style in
/// ems of the run size. Per hard-broken line that contains slots, the line's ascent and descent
/// are the maxima over its runs; the text baseline sits `(ascent - descent) / 2` below the
/// middle of the line box (the lift) and a slot with ascent `a` and descent `d` needs the half
/// height `max(a x scale - lift, d x scale + lift)`. The result is `max(configured,
/// ceil(2 x maximum half height x size))`. One forward pass over lines, slots and runs.
pub fn inline_line_height<S>(
    flat: &FlatText<S>,
    formulas: &[Arc<Formula>],
    scale: f32,
    configured: Pixels,
    size: Pixels,
    metrics: impl Fn(&S) -> (f32, f32),
) -> Pixels {
    if flat.slots.is_empty() {
        return configured;
    }
    let text = flat.text.as_bytes();
    let mut max_half = 0.0f32;
    let mut run_ix = 0usize;
    let mut run_start = 0usize;
    let mut slot_ix = 0usize;
    let mut line_start = 0usize;
    while line_start <= text.len() {
        let line_end = text[line_start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(text.len(), |offset| line_start + offset);
        // Slots starting in this line.
        let first_slot = slot_ix;
        while slot_ix < flat.slots.len() && flat.slots[slot_ix].start <= line_end {
            slot_ix += 1;
        }
        if slot_ix > first_slot {
            // Skip runs that end before the line.
            while run_ix < flat.runs.len() && run_start + flat.runs[run_ix].0 <= line_start {
                run_start += flat.runs[run_ix].0;
                run_ix += 1;
            }
            let (mut ascent, mut descent) = (0.0f32, 0.0f32);
            let mut scan_ix = run_ix;
            let mut scan_start = run_start;
            while scan_ix < flat.runs.len() && scan_start < line_end.max(line_start + 1) {
                let (len, style) = &flat.runs[scan_ix];
                let (a, d) = metrics(style);
                ascent = ascent.max(a);
                descent = descent.max(d);
                scan_start += len;
                scan_ix += 1;
            }
            let lift = (ascent - descent) / 2.0;
            for formula in &formulas[first_slot..slot_ix.min(formulas.len())] {
                let half = (formula.ascent() * scale - lift).max(formula.descent() * scale + lift);
                max_half = max_half.max(half);
            }
        }
        if line_end >= text.len() {
            break;
        }
        line_start = line_end + 1;
    }
    configured.max(px((2.0 * max_half * f32::from(size)).ceil()))
}

/// A paragraph under construction: displayed text with reserved slots, original text with the
/// canonical dollar-delimited sources and the map between them.
#[derive(Clone, Debug, Default)]
pub struct MathText {
    displayed: String,
    original: String,
    replacements: Vec<Replacement>,
    slots: Vec<Range<usize>>,
    formulas: Vec<Arc<Formula>>,
}

impl MathText {
    /// An empty paragraph.
    pub fn new() -> MathText {
        MathText::default()
    }

    /// Appends text shown as written.
    pub fn push_text(&mut self, text: &str) {
        self.displayed.push_str(text);
        self.original.push_str(text);
    }

    /// Appends an inline formula: the original text gets `$source$` (the canonical dollar form,
    /// whichever delimiters the document used) and the displayed text a run of reserved
    /// spaces. Returns `false` and appends nothing when the slot is too big to reserve.
    pub fn push_inline(&mut self, source: &str, formula: &Arc<Formula>, scale: f32) -> bool {
        self.push_slot(&format!("${source}$"), formula, scale)
    }

    fn push_slot(&mut self, original: &str, formula: &Arc<Formula>, scale: f32) -> bool {
        let Some(slot) = reserved_slot_text(formula, scale) else {
            return false;
        };
        let original_start = self.original.len();
        let displayed_start = self.displayed.len();
        self.original.push_str(original);
        self.displayed.push_str(&slot);
        self.replacements.push(Replacement::new(
            original_start..self.original.len(),
            displayed_start..self.displayed.len(),
        ));
        self.slots.push(displayed_start..self.displayed.len());
        self.formulas.push(formula.clone());
        true
    }

    /// A paragraph that is one display formula: its original text is `$$source$$`. `None` when
    /// the slot is too big to reserve.
    pub fn display(source: &str, formula: &Arc<Formula>, scale: f32) -> Option<MathText> {
        let mut text = MathText::new();
        text.push_slot(&format!("$${source}$$"), formula, scale)
            .then_some(text)
    }

    /// The displayed text.
    pub fn displayed(&self) -> &str {
        &self.displayed
    }

    /// The original text.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// Displayed byte range of every slot.
    pub fn slots(&self) -> &[Range<usize>] {
        &self.slots
    }

    /// The formula of every slot.
    pub fn formulas(&self) -> &[Arc<Formula>] {
        &self.formulas
    }

    /// Original text with its displayed form and the displayed-to-original map.
    pub fn original_text(&self) -> OriginalText {
        OriginalText::plain(self.original.as_str()).present(
            self.displayed.as_str(),
            DisplayMap::new(self.replacements.clone()),
        )
    }

    /// The flattened text for line-height and label helpers: the displayed text with styled
    /// runs (lengths in bytes, covering the displayed text) and the slot ranges.
    pub fn flat<S>(&self, runs: Vec<(usize, S)>) -> FlatText<S> {
        FlatText {
            text: self.displayed.clone(),
            runs,
            links: Vec::new(),
            code: Vec::new(),
            slots: self.slots.clone(),
        }
    }
}

/// How a formula is presented (CMP-012 R6): the fallback is never an error API.
#[derive(Clone, Debug)]
pub enum Presentation {
    /// A typeset formula with its reserved slot text.
    Formula {
        /// The laid-out formula.
        formula: Arc<Formula>,
        /// The reserved slot text.
        slot: String,
    },
    /// An inline formula shown as inline code with the dollar delimiters.
    InlineCode(String),
    /// A display formula shown as a code block labelled `math`.
    CodeBlock {
        /// The language label: always `math`.
        language: &'static str,
        /// The source text.
        text: String,
    },
}

/// The presentation of an inline formula under `style`: typeset unless math is disabled, the
/// engine refused the source, the height times the scale exceeds 12 em, or the extent is out of
/// range.
pub fn present_inline(source: &str, style: &MathStyle) -> Presentation {
    let code = || Presentation::InlineCode(format!("${source}$"));
    if !style.enabled {
        return code();
    }
    let Some(formula) = super::cache::lookup(source, false, style.face) else {
        return code();
    };
    let height = formula.height() * style.scale();
    if height.is_nan() || height > MAX_INLINE_HEIGHT_EM {
        return code();
    }
    match reserved_slot_text(&formula, style.scale()) {
        Some(slot) => Presentation::Formula { formula, slot },
        None => code(),
    }
}

/// The presentation of a display formula under `style` (the 12 em limit does not apply).
pub fn present_display(source: &str, style: &MathStyle) -> Presentation {
    let block = || Presentation::CodeBlock {
        language: "math",
        text: source.to_string(),
    };
    if !style.enabled {
        return block();
    }
    let Some(formula) = super::cache::lookup(source, true, style.face) else {
        return block();
    };
    match reserved_slot_text(&formula, style.scale()) {
        Some(slot) => Presentation::Formula { formula, slot },
        None => block(),
    }
}
