//! The Markdown layer of the multiline editor (CMP-009 R2-R7, R11-R13): the layer registers
//! itself as the editor's projection (replacements from the decorations), style spans (faces and
//! fenced-code syntax colors), newline hook and list-indent hook, so the editor needs no
//! Markdown knowledge.

use super::{Face, IndentEdit, MarkdownOptions, list_lines, list_prefix_display_len, newline_edit};
use crate::text::area::{AreaLayer, LayerEdit, LayerEnv, LayerIndent, Replacement, SpanStyle};
use std::{cell::RefCell, collections::HashSet, ops::Range};

/// The Markdown layer; attach it with
/// [`TextArea::with_markdown`](crate::text::TextArea::with_markdown).
#[derive(Default)]
pub struct MarkdownLayer {
    options: MarkdownOptions,
    decorations: RefCell<Vec<(Range<usize>, String)>>,
    list_starts: RefCell<HashSet<usize>>,
}

impl MarkdownLayer {
    /// A layer with the given options.
    pub fn new(options: MarkdownOptions) -> Self {
        Self {
            options,
            decorations: RefCell::default(),
            list_starts: RefCell::default(),
        }
    }

    /// The options.
    pub fn options(&self) -> MarkdownOptions {
        self.options
    }
}

fn as_layer_indent(edit: IndentEdit) -> LayerIndent {
    LayerIndent {
        edit: LayerEdit {
            range: edit.range,
            text: edit.text,
        },
        selection: edit.selection,
        reversed: edit.reversed,
    }
}

impl AreaLayer for MarkdownLayer {
    fn version(&self) -> u64 {
        self.options.processing_limit as u64 ^ u64::from(self.options.list_indent)
    }

    fn replacements(
        &self,
        source: &str,
        active: Range<usize>,
        _marked: Option<Range<usize>>,
    ) -> Vec<Replacement> {
        let decorations = self.options.decorations(source, active);
        *self.list_starts.borrow_mut() = list_lines(source)
            .into_iter()
            .map(|line| line.line_start)
            .collect();
        *self.decorations.borrow_mut() = decorations.clone();
        decorations
            .into_iter()
            .map(|(range, display)| Replacement {
                range,
                display,
                atomic: false,
            })
            .collect()
    }

    fn style_spans(&self, source: &str, env: &LayerEnv) -> Vec<(Range<usize>, SpanStyle)> {
        let mut spans: Vec<(Range<usize>, SpanStyle)> = self
            .options
            .faces(source)
            .into_iter()
            .map(|(range, face)| {
                let style = match face {
                    Face::Bold => SpanStyle::bold(),
                    Face::Italic => SpanStyle::italic(),
                    Face::Code => SpanStyle {
                        monospace: true,
                        color: Some(env.code_text),
                        background: Some(env.code_wash),
                        ..SpanStyle::default()
                    },
                    Face::Strikethrough => SpanStyle {
                        strikethrough: true,
                        ..SpanStyle::default()
                    },
                };
                (range, style)
            })
            .collect();
        for span in self.options.syntax_spans(source) {
            if let Some(color) = env.syntax.category(span.kind.category_key()) {
                spans.push((span.range, SpanStyle::color(color)));
            }
        }
        spans
    }

    fn newline_edit(&self, source: &str, caret: usize) -> Option<LayerEdit> {
        newline_edit(source, caret).map(|(range, text)| LayerEdit { range, text })
    }

    fn indent_edit(
        &self,
        source: &str,
        selection: Range<usize>,
        reversed: bool,
        outdent: bool,
    ) -> Option<LayerIndent> {
        self.options
            .indent_edit(source, selection, reversed, outdent)
            .map(as_layer_indent)
    }

    fn hanging_prefix_len(
        &self,
        source: &str,
        line_source: Range<usize>,
        _display_line: &str,
    ) -> Option<usize> {
        if !self.list_starts.borrow().contains(&line_source.start) {
            return None;
        }
        let line = source.get(line_source.clone())?;
        list_prefix_display_len(line, line_source.start, &self.decorations.borrow())
    }

    fn handles_tab(&self) -> bool {
        self.options.list_indent
    }
}
