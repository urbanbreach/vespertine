//! The read-only code block host (CMP-014 R1-R7, R9).
//!
//! A [`CodeBlock`] owns glyphs, syntax colors and optional line numbers; the caller owns the
//! surrounding card surface (the host's own style sets the outer surface and the typography).
//! Every logical line is one fixed-height, non-shrinking row (or a minimum-height row when the
//! block wraps), the content never wraps and is panned by an internal horizontal scroller that
//! is a child of the outer surface, so a horizontal pan never moves the surface's reported
//! bounds and a vertical wheel gesture continues to the enclosing scroller.

use std::hash::Hash;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    AnyElement, App, Bounds, Div, ElementId, InteractiveElement, Interactivity, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, Stateful, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, canvas, div, px,
};
use vespertine_richtext::syntax::{
    HighlightCache, HighlightedDocument, HighlighterAdapter, LanguageHint,
};
use vespertine_richtext::{CodeLine, RichText, RunKey};
use vespertine_theme::document::DocumentTheme;

use super::indicator::{CodeScrollState, scroll_indicator};
use super::lines::{gutter_width, line_ranges, normalize};
use super::typography::{CodeTypography, TypographyOverrides};
use crate::doc_theme::{alpha, hsla, syntax_colors};

/// Per-line gutter content supplied by the owner (R7).
#[derive(Default)]
pub struct GutterLine {
    /// Replaces the displayed number of the line (the source-line number of the line).
    pub number: Option<usize>,
    /// Extra content shown in the gutter next to the number (markers); never changes text
    /// advances.
    pub marker: Option<AnyElement>,
}

/// A click on a line or its number: the line and the source line it maps to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineClick {
    /// Zero-based line index.
    pub index: usize,
    /// The source-line number of the line: the gutter number override, else `index + 1`.
    pub source_line: usize,
    /// The text of the line.
    pub text: SharedString,
    /// Whether the line number (rather than the line text) was clicked.
    pub on_number: bool,
}

/// The bounds a line reports to the owner (R7, R14).
#[derive(Clone, Debug, PartialEq)]
pub struct LineBounds {
    /// Zero-based line index.
    pub index: usize,
    /// The source-line number of the line.
    pub source_line: usize,
    /// Row bounds in window coordinates.
    pub bounds: Bounds<Pixels>,
}

type GutterFn = Rc<dyn Fn(usize) -> GutterLine>;
type OverlayFn = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type ClickFn = Rc<dyn Fn(&LineClick, &mut Window, &mut App)>;
type BoundsFn = Rc<dyn Fn(&LineBounds)>;

enum Highlight {
    Cache(Arc<HighlightCache>),
    Document(Option<Arc<HighlightedDocument>>),
}

/// The code block host. Construct with [`CodeBlock::new`].
#[derive(IntoElement)]
pub struct CodeBlock {
    id: ElementId,
    root: Stateful<Div>,
    code: String,
    language: Option<String>,
    path: Option<String>,
    show_line_numbers: bool,
    theme: DocumentTheme,
    highlight: Highlight,
    wrap: bool,
    scroll: Option<CodeScrollState>,
    indicator: bool,
    key_owner: Option<u64>,
    gutter: Option<GutterFn>,
    overlays: Vec<(usize, OverlayFn)>,
    rows_after: Vec<(usize, OverlayFn)>,
    on_line_click: Option<ClickFn>,
    on_line_bounds: Option<BoundsFn>,
    on_bounds: Option<Rc<dyn Fn(Bounds<Pixels>)>>,
}

impl CodeBlock {
    /// A block of `code` (CRLF is normalized to LF). `id` identifies the block: its rows, its
    /// default scroll state and (unless [`CodeBlock::key_owner`] is set) the selection identity
    /// of its lines derive from it.
    pub fn new(id: impl Into<ElementId>, code: impl AsRef<str>) -> Self {
        let id = id.into();
        Self {
            root: div().id(id.clone()),
            id,
            code: normalize(code.as_ref()),
            language: None,
            path: None,
            show_line_numbers: false,
            theme: DocumentTheme::dark(),
            highlight: Highlight::Cache(HighlightCache::global()),
            wrap: false,
            scroll: None,
            indicator: false,
            key_owner: None,
            gutter: None,
            overlays: Vec::new(),
            rows_after: Vec::new(),
            on_line_click: None,
            on_line_bounds: None,
            on_bounds: None,
        }
    }

    /// The explicit language tag (wins over the path).
    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// The file path used to detect the language when there is no explicit tag.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Whether the gutter shows line numbers (default false).
    pub fn show_line_numbers(mut self, show: bool) -> Self {
        self.show_line_numbers = show;
        self
    }

    /// The theme the block reads colors, typography and gutter metrics from.
    pub fn theme(mut self, theme: DocumentTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Looks the highlight up in `cache` instead of the global one.
    pub fn highlight_cache(mut self, cache: Arc<HighlightCache>) -> Self {
        self.highlight = Highlight::Cache(cache);
        self
    }

    /// Uses an owner-supplied highlight for the block's text (`None` renders plain).
    pub fn highlight(mut self, document: Option<Arc<HighlightedDocument>>) -> Self {
        self.highlight = Highlight::Document(document);
        self
    }

    /// Wrapped fit-to-width rows (minimum row height, normal white-space) instead of the
    /// default no-wrap horizontally scrolling rows (R8 wrap toggle).
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// The owner-controlled scroll state of the block (keyed by the owner, for example by block
    /// index); without it the state lives as long as the block keeps rendering under its id.
    pub fn scroll_state(mut self, state: CodeScrollState) -> Self {
        self.scroll = Some(state);
        self
    }

    /// Shows the overlay scroll indicator over the bottom edge (R9).
    pub fn scroll_indicator(mut self, show: bool) -> Self {
        self.indicator = show;
        self
    }

    /// Replaces the owner identity of the line selection keys: line `i` registers as
    /// `RunKey::new(owner, i)`, so a document that renders the block under changing element ids
    /// keeps a stable selection.
    pub fn key_owner(mut self, owner: impl Hash) -> Self {
        self.key_owner = Some(RunKey::new(owner, 0).owner);
        self
    }

    /// Supplies per-line gutter content and the line-number override (R7).
    pub fn gutter(mut self, gutter: impl Fn(usize) -> GutterLine + 'static) -> Self {
        self.gutter = Some(Rc::new(gutter));
        self
    }

    /// Anchors an overlay to the bounds of line `line`: it is positioned absolutely over the row
    /// and never shifts glyph x positions.
    pub fn line_overlay(
        mut self,
        line: usize,
        overlay: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.overlays.push((line, Rc::new(overlay)));
        self
    }

    /// Inserts an additional row after line `line` (a review card). The row spans the block.
    pub fn row_after(
        mut self,
        line: usize,
        row: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.rows_after.push((line, Rc::new(row)));
        self
    }

    /// Raises a navigation event with the source line when a line or its number is clicked.
    pub fn on_line_click(
        mut self,
        handler: impl Fn(&LineClick, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_line_click = Some(Rc::new(handler));
        self
    }

    /// Reports every line's row bounds and source line each frame.
    pub fn on_line_bounds(mut self, handler: impl Fn(&LineBounds) + 'static) -> Self {
        self.on_line_bounds = Some(Rc::new(handler));
        self
    }

    /// Reports the bounds of the outer styled surface each frame (they never change while the
    /// internal viewport pans).
    pub fn on_bounds(mut self, handler: impl Fn(Bounds<Pixels>) + 'static) -> Self {
        self.on_bounds = Some(Rc::new(handler));
        self
    }

    fn overrides(style: &StyleRefinement, rem: Pixels) -> TypographyOverrides {
        let text = &style.text;
        let size = text.font_size.map(|size| size.to_pixels(rem));
        TypographyOverrides {
            family: text.font_family.clone(),
            weight: text.font_weight,
            size,
            line_height: text.line_height.map(|height| {
                height.to_pixels(
                    text.font_size
                        .unwrap_or_else(|| vespertine::AbsoluteLength::Pixels(px(0.))),
                    rem,
                )
            }),
            color: text.color,
        }
    }
}

impl Styled for CodeBlock {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}

impl InteractiveElement for CodeBlock {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.interactivity()
    }
}

impl StatefulInteractiveElement for CodeBlock {}

impl RenderOnce for CodeBlock {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let typography = CodeTypography::resolve(
            &Self::overrides(self.root.style(), window.rem_size()),
            &self.theme,
        );
        let ranges = line_ranges(&self.code);
        let line_count = ranges.len();
        let colors = syntax_colors(&self.theme).with_plain(typography.color);
        let hint = LanguageHint {
            fence_label: self.language.as_deref(),
            path: self.path.as_deref(),
        };
        let document = match &self.highlight {
            Highlight::Document(document) => document.clone(),
            Highlight::Cache(cache) => cache.document_for(&self.code, hint),
        };
        let font = typography.font();
        let line_height = typography.line_height;
        let owner = self
            .key_owner
            .unwrap_or_else(|| RunKey::new(&self.id, 0).owner);
        let wrap = self.wrap;
        let gutter_callback = self.gutter.clone();
        let source_line = |index: usize, number: Option<usize>| number.unwrap_or(index + 1);

        let mut rows: Vec<AnyElement> = Vec::with_capacity(line_count);
        let mut gutter_cells: Vec<AnyElement> = Vec::with_capacity(line_count);
        let gutter_px = gutter_width(line_count, &self.theme.metrics);
        let faint = hsla(self.theme.text_faint);
        let padding_right = self.theme.metrics.code_gutter_padding_right;

        for (index, range) in ranges.iter().enumerate() {
            let text = &self.code[range.clone()];
            let spans = document
                .as_deref()
                .and_then(|document| document.lines().get(index))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let line = CodeLine::new(
                index,
                text,
                spans
                    .iter()
                    .map(|span| (span.range.clone(), colors.color_for(span.kind))),
                typography.color,
                &font,
                line_height,
            );
            let GutterLine { number, marker } = gutter_callback
                .as_ref()
                .map(|gutter| gutter(index))
                .unwrap_or_default();
            let source = source_line(index, number);
            let shared_text = SharedString::from(text.to_owned());

            let mut row = div()
                .id(("code-row", index))
                .relative()
                .flex_none()
                .w_full()
                .child(
                    RichText::new(("code-line", index), line.text().to_owned())
                        .runs(line.runs().to_vec())
                        .sub_index(index as u32)
                        .key(RunKey {
                            owner,
                            sub: index as u32,
                        })
                        .native(),
                );
            row = if wrap {
                row.min_h(line_height)
            } else {
                row.h(line_height).whitespace_nowrap()
            };
            if let Some(handler) = &self.on_line_click {
                let handler = handler.clone();
                let click = LineClick {
                    index,
                    source_line: source,
                    text: shared_text.clone(),
                    on_number: false,
                };
                row = row.on_click(move |_, window, cx| handler(&click, window, cx));
            }
            if let Some(report) = &self.on_line_bounds {
                let report = report.clone();
                row = row.child(
                    canvas(
                        move |bounds, _, _| {
                            report(&LineBounds {
                                index,
                                source_line: source,
                                bounds,
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                );
            }
            for (_, overlay) in self.overlays.iter().filter(|(line, _)| *line == index) {
                let anchored = div().size_full().absolute().left_0().top_0();
                row = row.child(anchored.child(overlay(window, cx)));
            }
            rows.push(row.into_any_element());
            for (_, extra) in self.rows_after.iter().filter(|(line, _)| *line == index) {
                rows.push(
                    div()
                        .flex_none()
                        .w_full()
                        .child(extra(window, cx))
                        .into_any_element(),
                );
            }

            if self.show_line_numbers {
                let label: SharedString = source.to_string().into();
                let mut cell = div()
                    .id(("code-number", index))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .flex_none()
                    .w_full()
                    .pr(px(padding_right))
                    .child(
                        RichText::new(("code-number-text", index), label.to_string())
                            .chrome()
                            .runs(vec![vespertine::TextRun {
                                len: label.len(),
                                font: font.clone(),
                                color: faint,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }]),
                    );
                cell = if wrap {
                    cell.min_h(line_height)
                } else {
                    cell.h(line_height)
                };
                if let Some(marker) = marker {
                    cell = cell.child(marker);
                }
                if let Some(handler) = &self.on_line_click {
                    let handler = handler.clone();
                    let click = LineClick {
                        index,
                        source_line: source,
                        text: shared_text,
                        on_number: true,
                    };
                    cell = cell.on_click(move |_, window, cx| handler(&click, window, cx));
                }
                gutter_cells.push(cell.into_any_element());
                for (_, _) in self.rows_after.iter().filter(|(line, _)| *line == index) {
                    // Keep the gutter aligned with the inserted rows.
                    gutter_cells.push(div().flex_none().into_any_element());
                }
            }
        }

        let scroll = self.scroll.clone().unwrap_or_else(|| {
            window
                .use_keyed_state((self.id.clone(), "code-scroll"), cx, |_, _| {
                    CodeScrollState::new()
                })
                .read(cx)
                .clone()
        });

        let content = div()
            .flex()
            .flex_col()
            .flex_none()
            .min_w_full()
            .children(rows);
        let mut scroller = div()
            .id((self.id.clone(), "code-scroller"))
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .child(content);
        if !wrap {
            scroller = scroller
                .overflow_x_scroll()
                .restrict_scroll_to_axis()
                .track_scroll(scroll.handle());
        }

        let mut root = self
            .root
            .relative()
            .flex()
            .flex_row()
            .font_family(typography.family.clone())
            .text_size(typography.size)
            .line_height(typography.line_height)
            .font_weight(typography.weight)
            .text_color(typography.color);
        if self.show_line_numbers {
            root = root.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(px(gutter_px))
                    .text_color(alpha(faint, faint.a))
                    .children(gutter_cells),
            );
        }
        root = root.child(scroller);
        if let Some(report) = self.on_bounds.clone() {
            root = root.child(
                canvas(move |bounds, _, _| report(bounds), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            );
        }
        if self.indicator && !wrap {
            root = root.child(scroll_indicator(&scroll, faint));
        }
        root
    }
}
