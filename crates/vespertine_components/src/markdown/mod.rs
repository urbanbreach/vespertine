//! Source-preserving Markdown editing logic (CMP-009).
//!
//! The document is always plain Markdown source. This module is the pure layer behind the
//! editor's Markdown mode: it never rewrites the source, and every styling or display change is a
//! description over exact source byte ranges for the editor to project.
//!
//! * [`faces`]: bold, italic, code and strikethrough ranges (R2).
//! * [`syntax_spans`]: syntax colors of fenced code bodies (R8).
//! * [`decorations`]: replacements that hide delimiters and show glyphs (R4), computed for an
//!   [`active_range`].
//! * [`in_code`]: is an editing action inside code (R9)?
//! * [`list_prefix`], [`newline_edit`], [`indent_edit`], [`list_lines`] and
//!   [`list_prefix_display_len`]: Enter continuation, list indent and wrapped list items
//!   (R7, R11-R13).
//!
//! [`faces`], [`syntax_spans`] and [`decorations`] produce nothing for a source longer than the
//! processing limit (R14); the code-context and list queries are never limited. Use
//! [`MarkdownOptions`] methods for a configured limit; the free functions use the default.

mod code;
mod decorations;
mod highlight;
mod indent;
mod layer;
mod list;
mod newline;
mod parse;
mod text;

pub use code::in_code;
pub use indent::{IndentEdit, indent_edit};
pub use layer::MarkdownLayer;
pub use list::{ListPrefix, TaskBox, list_prefix, list_prefix_display_len};
pub use newline::{ListLine, list_lines, newline_edit};
pub use vespertine_richtext::syntax::TokenKind;

use std::ops::Range;

/// Default source limit for faces, decorations and syntax spans: 128 KiB (R14).
pub const DEFAULT_PROCESSING_LIMIT: usize = 131_072;
/// Most syntax spans produced across one document (R8).
pub const MAX_SYNTAX_SPANS: usize = 16_000;

/// Configuration of the Markdown layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkdownOptions {
    /// Sources longer than this many bytes get no faces, decorations or syntax spans. Default
    /// 131,072.
    pub processing_limit: usize,
    /// Whether Tab and Shift+Tab indent and outdent list lines. Default `true`.
    pub list_indent: bool,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            processing_limit: DEFAULT_PROCESSING_LIMIT,
            list_indent: true,
        }
    }
}

/// A text face of a Markdown element. Faces nest by depth counting, so `***both***` is bold and
/// italic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    /// Emphasis.
    Italic,
    /// Strong emphasis and headings.
    Bold,
    /// Strikethrough (`~` runs).
    Strikethrough,
    /// Inline code, fenced and indented code blocks.
    Code,
}

/// A syntax-colored byte range of a fenced code body: the neutral token kind of the TXT-007
/// highlighter and the exact source range, never including fence characters or container
/// prefixes and always on char boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    /// Source byte range.
    pub range: Range<usize>,
    /// Neutral token kind; the editor colors it from the theme's syntax roles.
    pub kind: TokenKind,
}

impl MarkdownOptions {
    /// Whether the layer styles `source` (it is not longer than the processing limit).
    pub fn applies_to(&self, source: &str) -> bool {
        source.len() <= self.processing_limit
    }

    /// Face ranges over `source` in document order (R2): the full parser range of each element,
    /// delimiters included. Empty above the processing limit.
    pub fn faces(&self, source: &str) -> Vec<(Range<usize>, Face)> {
        if !self.applies_to(source) {
            return Vec::new();
        }
        parse::Parsed::parse(source).faces
    }

    /// Syntax spans of the fenced code bodies of `source` (R8), sorted and non-overlapping, at
    /// most [`MAX_SYNTAX_SPANS`]. Empty above the processing limit.
    pub fn syntax_spans(&self, source: &str) -> Vec<HighlightSpan> {
        if !self.applies_to(source) {
            return Vec::new();
        }
        highlight::compute(source)
    }

    /// Replacements (source range, display text) for the lines outside `active` (R4), sorted by
    /// start and non-overlapping. `active` is the union of the logical lines intersecting the
    /// selection and any marked range, see [`active_range`]. Empty above the processing limit.
    pub fn decorations(&self, source: &str, active: Range<usize>) -> Vec<(Range<usize>, String)> {
        if !self.applies_to(source) {
            return Vec::new();
        }
        decorations::compute(source, active)
    }

    /// [`indent_edit`] unless list indentation is disabled in the options.
    pub fn indent_edit(
        &self,
        source: &str,
        selection: Range<usize>,
        reversed: bool,
        outdent: bool,
    ) -> Option<IndentEdit> {
        if !self.list_indent {
            return None;
        }
        indent_edit(source, selection, reversed, outdent)
    }
}

/// Face ranges over `source` with the default processing limit; see [`MarkdownOptions::faces`].
pub fn faces(source: &str) -> Vec<(Range<usize>, Face)> {
    MarkdownOptions::default().faces(source)
}

/// Syntax spans with the default processing limit; see [`MarkdownOptions::syntax_spans`].
pub fn syntax_spans(source: &str) -> Vec<HighlightSpan> {
    MarkdownOptions::default().syntax_spans(source)
}

/// Decorations with the default processing limit; see [`MarkdownOptions::decorations`].
pub fn decorations(source: &str, active: Range<usize>) -> Vec<(Range<usize>, String)> {
    MarkdownOptions::default().decorations(source, active)
}

/// The active range of R4: the union of the logical lines intersecting `selection` and the
/// `marked` range, as one range from the start of the first line to the end of the last line
/// (line terminators excluded). A non-empty range ending right after a line feed does not
/// include the following line; a collapsed caret belongs to its own line. Offsets past the end
/// are clamped to the source.
pub fn active_range(
    source: &str,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
) -> Range<usize> {
    let lines = text::lines_covering(source, selection);
    match marked {
        Some(marked) => {
            let marked = text::lines_covering(source, marked);
            lines.start.min(marked.start)..lines.end.max(marked.end)
        }
        None => lines,
    }
}

#[cfg(test)]
mod tests;
