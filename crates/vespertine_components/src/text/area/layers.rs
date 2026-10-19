//! Layers that plug into the multiline editor: Markdown styling and decorations (CMP-009) and
//! inline reference chips (CMP-010) supply replacements, style spans and editing hooks through
//! this interface so the editor itself needs no knowledge of either.

use super::{projection::Replacement, runs::SpanStyle};
use crate::text::common::FieldTheme;
use std::ops::Range;
use vespertine::{App, Hsla};
use vespertine_richtext::syntax::SyntaxColors;

/// The theme roles layers style with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerEnv {
    /// The editor's roles.
    pub theme: FieldTheme,
    /// The syntax palette of the active theme.
    pub syntax: SyntaxColors,
    /// The wash behind code.
    pub code_wash: Hsla,
    /// The text color of code.
    pub code_text: Hsla,
}

impl LayerEnv {
    /// The roles of the active application theme (neutral defaults before one is installed).
    pub fn resolve(theme: FieldTheme, cx: &App) -> Self {
        let mut syntax = SyntaxColors::dark();
        if let Some(active) = vespertine_theme::appearance::try_theme_of(cx) {
            syntax.plain = active.colors.text.to_hsla();
            for category in vespertine_theme::theme::SyntaxCategory::ALL {
                if let Some(color) = active.syntax.get(category) {
                    syntax.set_category(category.name(), color.to_hsla());
                }
            }
        }
        let mut wash = theme.text;
        wash.a = 0.10;
        Self {
            theme,
            syntax,
            code_wash: wash,
            code_text: theme.text,
        }
    }
}

/// An edit proposed by a layer: replace `range` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerEdit {
    /// The source range to replace.
    pub range: Range<usize>,
    /// The replacement text.
    pub text: String,
}

/// The result of a layer's list-indent operation (CMP-009 R13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerIndent {
    /// The edit to apply as one undo step; an empty edit is an eligible no-op that still
    /// consumes the key.
    pub edit: LayerEdit,
    /// The selection after the edit (source offsets).
    pub selection: Range<usize>,
    /// Whether the caret is at the selection start.
    pub reversed: bool,
}

/// A layer attached to a [`TextArea`](super::TextArea).
pub trait AreaLayer {
    /// A counter that changes whenever the layer's own configuration changed so the editor
    /// rebuilds its projection (for example when the set of live attachments changes).
    fn version(&self) -> u64 {
        0
    }

    /// Replacements over the source: `active` is the union of the logical lines intersecting
    /// the selection and `marked` the marked range. They are rebuilt after every edit,
    /// marked-range change and active-line change and never touch history (R17).
    fn replacements(
        &self,
        _source: &str,
        _active: Range<usize>,
        _marked: Option<Range<usize>>,
    ) -> Vec<Replacement> {
        Vec::new()
    }

    /// Style spans over the source (faces, syntax colors).
    fn style_spans(&self, _source: &str, _env: &LayerEnv) -> Vec<(Range<usize>, SpanStyle)> {
        Vec::new()
    }

    /// The newline hook (R7): an optional replacement edit for Enter at a collapsed caret.
    fn newline_edit(&self, _source: &str, _caret: usize) -> Option<LayerEdit> {
        None
    }

    /// The list-indent hook (CMP-009 R13); `None` lets Tab propagate to focus traversal.
    fn indent_edit(
        &self,
        _source: &str,
        _selection: Range<usize>,
        _reversed: bool,
        _outdent: bool,
    ) -> Option<LayerIndent> {
        None
    }

    /// The length in DISPLAY bytes of the prefix of a logical line that continuation rows are
    /// indented by (CMP-009 R7); `None` for lines without a hanging indent. `line_source` is the
    /// line's source range and `display_line` its projected text.
    fn hanging_prefix_len(
        &self,
        _source: &str,
        _line_source: Range<usize>,
        _display_line: &str,
    ) -> Option<usize> {
        None
    }

    /// Whether Markdown-style Tab handling is active (the editor binds Tab/Shift+Tab to the
    /// indent hook only then).
    fn handles_tab(&self) -> bool {
        false
    }
}
