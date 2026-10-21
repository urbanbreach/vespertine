//! The `CodeView` element: syntax-colored source with one line element per source line
//! (TXT-007 R10, R13, R17).
//!
//! Every line is a native [`RichText`] run whose run list comes from [`runs_from_spans`] with ONE
//! shared font, so changing colors (or receiving a late highlight result) never changes shaping.
//! Inside a [`crate::text_root`] each line registers as one selectable run (sub-index = line
//! index) and takes part in search washes, matched against the exact string it paints.

use super::cache::{HighlightCache, HighlighterAdapter, LanguageHint, RetainedHighlight};
use super::document::{HighlightedDocument, line_contents};
use super::theme::SyntaxColors;
use crate::RichText;
use crate::runs::runs_from_spans;
use std::ops::Range;
use std::sync::Arc;
use vespertine::{
    App, ElementId, Font, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    Refineable, RenderOnce, SharedString, StyleRefinement, Styled, TextRun, Window, div, font, px,
};

/// Where a [`CodeView`] gets its document from.
enum Highlight {
    /// Look the source up in this cache when the view renders.
    Cache(Arc<HighlightCache>),
    /// An already resolved document (`None` renders plain).
    Document(Option<Arc<HighlightedDocument>>),
}

/// Syntax-colored source text: no surface, padding or line numbers by default, an explicit font
/// family, size, line height and weight, the plain foreground for unclassified text, and a
/// [`SyntaxColors`] palette. Size and position are given through the [`Styled`] methods.
#[derive(IntoElement)]
pub struct CodeView {
    source: Arc<str>,
    language: Option<String>,
    path: Option<String>,
    highlight: Highlight,
    colors: SyntaxColors,
    font_family: SharedString,
    font_size: Pixels,
    line_height: Pixels,
    font_weight: FontWeight,
    id: ElementId,
    style: StyleRefinement,
}

impl CodeView {
    /// A view of `source` using the global cache, the dark palette, a 12.5 px monospace font
    /// with line height 18 and regular weight.
    #[track_caller]
    pub fn new(source: impl Into<Arc<str>>) -> Self {
        Self {
            id: ElementId::from(std::panic::Location::caller()),
            source: source.into(),
            language: None,
            path: None,
            highlight: Highlight::Cache(HighlightCache::global()),
            colors: SyntaxColors::dark(),
            font_family: SharedString::from("monospace"),
            font_size: px(12.5),
            line_height: px(18.),
            font_weight: FontWeight::NORMAL,
            style: StyleRefinement::default(),
        }
    }

    /// Identity of the view: line runs are keyed by it, so two views painted together in one
    /// document need distinct ids (the default is the construction site).
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// The language alias (the host `language` property).
    pub fn language(mut self, alias: impl Into<String>) -> Self {
        self.language = Some(alias.into());
        self
    }

    /// The fence label of a document fence (a language alias, same as [`CodeView::language`]).
    pub fn fence_label(self, label: impl Into<String>) -> Self {
        self.language(label)
    }

    /// The file path (the host `path` property) used to detect the language.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Looks the source up in `cache` instead of the global one when the view renders.
    pub fn cache(mut self, cache: Arc<HighlightCache>) -> Self {
        self.highlight = Highlight::Cache(cache);
        self
    }

    /// Uses this resolved document (`None` renders plain). The document must belong to the
    /// source; a document with fewer lines leaves the remaining lines plain and spans past a
    /// shortened line are clipped (TXT-006 R2).
    pub fn document(mut self, document: Option<Arc<HighlightedDocument>>) -> Self {
        self.highlight = Highlight::Document(document);
        self
    }

    /// Resolves the document through the widget's retained handle (TXT-007 R17): while the
    /// source, language, path and cache generation are unchanged this performs no lookup and no
    /// hashing. Call it after [`CodeView::language`] and [`CodeView::path`].
    pub fn retained(self, retained: &mut RetainedHighlight, cache: &HighlightCache) -> Self {
        let hint = LanguageHint {
            fence_label: self.language.as_deref(),
            path: self.path.as_deref(),
        };
        let document = retained.resolve(cache, &self.source, hint).cloned();
        Self {
            highlight: Highlight::Document(document),
            ..self
        }
    }

    /// The palette used to color spans.
    pub fn colors(mut self, colors: SyntaxColors) -> Self {
        self.colors = colors;
        self
    }

    /// Font family of every line.
    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = family.into();
        self
    }

    /// Font size of every line.
    pub fn font_size(mut self, size: impl Into<Pixels>) -> Self {
        self.font_size = size.into();
        self
    }

    /// Height of every line.
    pub fn line_height(mut self, height: impl Into<Pixels>) -> Self {
        self.line_height = height.into();
        self
    }

    /// Font weight of every line.
    pub fn font_weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = weight;
        self
    }

    fn resolve_document(&self) -> Option<Arc<HighlightedDocument>> {
        match &self.highlight {
            Highlight::Document(document) => document.clone(),
            Highlight::Cache(cache) => cache.document_for(
                &self.source,
                LanguageHint {
                    fence_label: self.language.as_deref(),
                    path: self.path.as_deref(),
                },
            ),
        }
    }

    /// The ordered `(line text, runs)` of the view for the given palette and font; one entry per
    /// source line.
    pub fn lines(&self) -> Vec<CodeLine> {
        let document = self.resolve_document();
        let font = self.font();
        line_contents(&self.source)
            .into_iter()
            .enumerate()
            .map(|(index, range)| {
                let spans = document
                    .as_deref()
                    .and_then(|document| document.lines().get(index))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                CodeLine::new(
                    index,
                    &self.source[range],
                    spans
                        .iter()
                        .map(|span| (span.range.clone(), self.colors.color_for(span.kind))),
                    self.colors.plain,
                    &font,
                    self.line_height,
                )
            })
            .collect()
    }

    fn font(&self) -> Font {
        let mut font = font(self.font_family.clone());
        font.weight = self.font_weight;
        font
    }
}

impl Styled for CodeView {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for CodeView {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let lines = self.lines();
        let mut root = div()
            .id(self.id.clone())
            .flex()
            .flex_col()
            .overflow_hidden()
            .font_family(self.font_family.clone())
            .text_size(self.font_size)
            .line_height(self.line_height)
            .font_weight(self.font_weight)
            .text_color(self.colors.plain);
        root.style().refine(&self.style);
        root.children(lines)
    }
}

/// One painted line of a [`CodeView`]: its text, the runs built from the line's spans with the
/// view's single font, and the line height. Line painting lives here so selection and search
/// layers can attach to it.
#[derive(IntoElement)]
pub struct CodeLine {
    index: u32,
    text: SharedString,
    runs: Vec<TextRun>,
    height: Pixels,
}

impl CodeLine {
    /// Builds a line from its text and `(byte range, color)` spans sorted by start; every run
    /// carries `font`, gaps get `plain` (TXT-006 R2/R3).
    pub fn new(
        index: usize,
        text: &str,
        spans: impl IntoIterator<Item = (Range<usize>, Hsla)>,
        plain: Hsla,
        font: &Font,
        height: Pixels,
    ) -> Self {
        let spans: Vec<(Range<usize>, Hsla)> = spans.into_iter().collect();
        Self {
            index: index as u32,
            runs: runs_from_spans(text, &spans, plain, font),
            text: SharedString::from(text.to_owned()),
            height,
        }
    }

    /// The index of the line in the source.
    pub fn index(&self) -> usize {
        self.index as usize
    }

    /// The text of the line (without its terminator).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The runs covering the text exactly.
    pub fn runs(&self) -> &[TextRun] {
        &self.runs
    }
}

impl RenderOnce for CodeLine {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().h(self.height).flex_none().whitespace_nowrap().child(
            RichText::new(
                ElementId::from(("code-line", self.index as usize)),
                self.text.to_string(),
            )
            .runs(self.runs)
            .sub_index(self.index)
            .native(),
        )
    }
}
