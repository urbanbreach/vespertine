//! Inline and display mathematical notation (CMP-012).
//!
//! A formula is laid out once, in em units, by a TeX-style algorithm driven by the OpenType MATH
//! table of the selected face ([`layout`]), and then scaled to any text size without laying out
//! again. Inline formulas reserve measured space in the text flow with a run of no-break spaces
//! ([`slot`]) so that text shaping, wrapping, selection, copy and pointer mapping stay those of
//! ordinary text; display formulas are one-line blocks of their own. Invalid, disabled, too tall
//! or too wide formulas fall back to readable source code ([`slot::present_inline`],
//! [`slot::present_display`]). Layout results, including failures, live in a bounded
//! process-wide cache ([`cache`]).
//!
//! # Using it from a document renderer
//!
//! ```ignore
//! let style = MathStyle::new();                       // enabled, default face, 100 %
//! match present_inline("\\frac{a}{b}", &style) {
//!     Presentation::Formula { formula, .. } => paragraph.push_inline(src, &formula, style.scale()),
//!     Presentation::InlineCode(code) => /* inline code span with `code` */,
//!     Presentation::CodeBlock { .. } => unreachable!(),
//! }
//! let line_height = inline_line_height(&flat, paragraph.formulas(), style.scale(), configured, size, metrics);
//! let element = math_text(id, &paragraph, slot_runs(&paragraph, &run), MathTextOptions::inline(style.scale()), window, cx);
//! ```

pub mod cache;
pub mod faces;
pub mod formula;
pub mod layout;
pub mod paint;
pub mod parse;
pub mod slot;
mod symbols;

pub use cache::{FormulaCache, MAX_BYTES, MAX_ENTRIES, lookup};
pub use faces::{FACE_COUNT, FACE_TABLE, FaceInfo, MathFace, SLOT_REFERENCE_FACE};
pub use formula::{DisplayItem, Formula, MathStyle};
pub use paint::{
    DisplayBlock, FaceStatus, MathBlock, MathFonts, MathPainter, MathTextOptions, PaintObserver,
    PaintReport, ShapedSample, SlotPaint, SlotReport, ensure_face, math_text, paint_formula,
    slot_font, slot_runs, verify_face,
};
pub use parse::MathError;
pub use slot::{
    DISPLAY_PAD, MAX_INLINE_HEIGHT_EM, MAX_SLOT_EM, MathText, Presentation, RESERVED_SPACE,
    display_line_height, inline_line_height, present_display, present_inline, reserved_slot_text,
    slot_chars, slot_text_for_extent, space_advance,
};
