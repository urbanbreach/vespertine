//! The composed comparison viewer (CMP-015 R30-R37).
//!
//! [`ComparisonView`] takes already-structured hunks and shows a one-column stream or aligned
//! before/after columns, built from ordinary container and text elements. It has no virtual
//! scrolling and no events; the host supplies scrolling. Word washes are similarity gated, syntax
//! grammar state advances independently along the before and after streams, and the per-hunk
//! line counters keep their deliberately preserved behavior (R37, see [`CompareModel::build`]).

use super::jsdiff::{PartKind, diff_words};
use std::collections::HashMap;
use std::sync::Arc;
use vespertine::{
    App, Div, Hsla, IntoElement, ParentElement, Pixels, Refineable, RenderOnce, Rgba, SharedString,
    StyleRefinement, Styled, Window, div, px,
};
use vespertine_richtext::syntax::{
    HighlightCache, Language, Limits, Span, SyntaxColors, TokenKind,
};

/// One structured hunk (R30): the old and new start and line counts and the lines, each a string
/// whose first character is `+`, `-` or a space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComparisonHunk {
    /// First old line of the hunk.
    pub old_start: u32,
    /// Number of old lines.
    pub old_count: u32,
    /// First new line of the hunk.
    pub new_start: u32,
    /// Number of new lines.
    pub new_count: u32,
    /// Lines with their marker character first.
    pub lines: Vec<String>,
}

/// A word-level change of a token (R31).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordChange {
    /// A removed token (on the removal line).
    Removed,
    /// An added token (on the addition line).
    Added,
}

/// One text token of a line: plain text, a syntax-colored span or a word-diff token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// The text.
    pub text: String,
    /// The syntax kind (colors only); `None` renders in the plain glyph color.
    pub syntax: Option<TokenKind>,
    /// Set for tokens of a word-diff line: plain glyph color on a changed-token background.
    pub word: Option<WordChange>,
}

/// What a cell shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    /// A context line.
    Context,
    /// A removal.
    Removal,
    /// An addition.
    Addition,
}

/// One gutter-and-body cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Context, removal or addition.
    pub kind: CellKind,
    /// The number the gutter shows (R33, R37).
    pub counter: u32,
    /// The text tokens of the body.
    pub tokens: Vec<Token>,
}

/// One item of the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompareItem {
    /// The separator between hunks (R36).
    Separator(String),
    /// A row of the one-column stream (R33).
    Unified(Cell),
    /// A row of the aligned columns (R34); a missing side is an empty cell.
    Split {
        /// Left (before) cell.
        left: Option<Cell>,
        /// Right (after) cell.
        right: Option<Cell>,
    },
}

/// The processed model of a comparison: gutter widths and items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareModel {
    /// Digit width of the left (old) gutter (at least 2).
    pub left_width: usize,
    /// Digit width of the right (new) gutter (at least 2).
    pub right_width: usize,
    /// The rows in patch order.
    pub items: Vec<CompareItem>,
}

/// Similarity of two strings: `(L - d) / L` with `L` the longer length in UTF-16 code units and
/// `d` the Levenshtein distance; two empty strings give 1 (R31).
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.;
    }
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    let mut previous: Vec<usize> = (0..=short.len()).collect();
    let mut current = vec![0; short.len() + 1];
    for (i, long_unit) in long.iter().enumerate() {
        current[0] = i + 1;
        for (j, short_unit) in short.iter().enumerate() {
            let substitution = previous[j] + usize::from(long_unit != short_unit);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let distance = previous[short.len()];
    (longest - distance) as f64 / longest as f64
}

/// The language of a file path for the composed viewer (R32): by extension, anything else (and a
/// missing path) is JavaScript. `None` when the extension names a language the shared syntax
/// service has no grammar for (java, sql): those render in the plain glyph color.
pub fn language_for_path(path: Option<&str>) -> Option<Language> {
    let extension = path
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase());
    Some(match extension.as_deref() {
        Some("ts") => Language::TypeScript,
        Some("tsx") => Language::Tsx,
        Some("jsx") => Language::Jsx,
        Some("js" | "mjs" | "cjs") => Language::JavaScript,
        Some("json") => Language::Json,
        Some("md" | "mdx" | "markdown") => Language::Markdown,
        Some("html" | "htm") => Language::Html,
        Some("css") => Language::Css,
        Some("py") => Language::Python,
        Some("rs") => Language::Rust,
        Some("go") => Language::Go,
        Some("c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx") => Language::C,
        Some("yaml" | "yml") => Language::Yaml,
        Some("toml") => Language::Toml,
        Some("sh" | "bash") => Language::Shell,
        Some("java" | "sql") => return None,
        _ => Language::JavaScript,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Context,
    Removal,
    Addition,
}

struct ParsedLine<'a> {
    kind: Kind,
    text: &'a str,
}

fn parse_line(line: &str) -> ParsedLine<'_> {
    let mut chars = line.chars();
    let kind = match chars.next() {
        Some('+') => Kind::Addition,
        Some('-') => Kind::Removal,
        _ => Kind::Context,
    };
    ParsedLine {
        kind,
        text: chars.as_str(),
    }
}

fn digits(value: u32) -> usize {
    value.max(1).ilog10() as usize + 1
}

/// Tokens of one line from its syntax spans: spans in their kind, gaps plain.
fn syntax_tokens(text: &str, spans: &[Span]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    for span in spans {
        let start = span.range.start.max(cursor).min(text.len());
        let end = span.range.end.min(text.len());
        if end <= start || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        if start > cursor {
            tokens.push(Token {
                text: text[cursor..start].to_owned(),
                syntax: None,
                word: None,
            });
        }
        tokens.push(Token {
            text: text[start..end].to_owned(),
            syntax: Some(span.kind),
            word: None,
        });
        cursor = end;
    }
    if cursor < text.len() {
        tokens.push(Token {
            text: text[cursor..].to_owned(),
            syntax: None,
            word: None,
        });
    }
    tokens
}

fn plain_tokens(text: &str) -> Vec<Token> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![Token {
            text: text.to_owned(),
            syntax: None,
            word: None,
        }]
    }
}

/// Highlights one stream (lines joined with LF), one span list per stream line. Grammar state
/// advances along the stream: each line is tokenized as a continuation of the previous one (R32).
fn stream_spans(
    cache: &HighlightCache,
    language: Option<Language>,
    texts: &[&str],
) -> Vec<Vec<Span>> {
    let Some(language) = language else {
        return vec![Vec::new(); texts.len()];
    };
    let source = texts.join("\n");
    let document = cache.highlight_as(language, &source, Limits::default());
    (0..texts.len())
        .map(|index| {
            document
                .as_ref()
                .and_then(|doc| doc.lines().get(index))
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

struct Snapshot {
    old: u32,
    new: u32,
}

impl CompareModel {
    /// Processes the hunks (R31-R37).
    ///
    /// QUIRK (R37, preserved deliberately; the native patch viewer keeps correct per-side
    /// counters): both displayed counters of a hunk start at that hunk's OLD start line. The
    /// old counter advances on removals and context, the new counter on additions and context.
    /// A removal shows the current old counter and the non-advancing current new counter; an
    /// addition shows the new counter value it had when processed. So whenever a hunk's new
    /// start differs from its old start, displayed new-side numbers are offset by the
    /// difference, and for an old start of 0 the first new-side number is 0. The overall gutter
    /// widths of R35 still derive from the first hunk's start values.
    pub fn build(
        hunks: &[ComparisonHunk],
        file_path: Option<&str>,
        split: bool,
        cache: &HighlightCache,
    ) -> Self {
        let language = language_for_path(file_path);
        let mut old_total = hunks.first().map_or(0, |hunk| {
            if hunk.old_start == 0 {
                1
            } else {
                hunk.old_start
            }
        });
        let mut new_total = hunks.first().map_or(0, |hunk| {
            if hunk.new_start == 0 {
                1
            } else {
                hunk.new_start
            }
        });
        for hunk in hunks {
            for line in &hunk.lines {
                match parse_line(line).kind {
                    Kind::Removal => old_total += 1,
                    Kind::Addition => new_total += 1,
                    Kind::Context => {
                        old_total += 1;
                        new_total += 1;
                    }
                }
            }
        }
        let left_width = digits(old_total).max(2);
        let right_width = digits(new_total).max(2);
        let mut items = Vec::new();
        for (index, hunk) in hunks.iter().enumerate() {
            if index > 0 {
                let width = left_width.max(right_width) + 2;
                items.push(CompareItem::Separator(format!("{}...", " ".repeat(width))));
            }
            Self::push_hunk(&mut items, hunk, language, split, cache);
        }
        Self {
            left_width,
            right_width,
            items,
        }
    }

    fn push_hunk(
        items: &mut Vec<CompareItem>,
        hunk: &ComparisonHunk,
        language: Option<Language>,
        split: bool,
        cache: &HighlightCache,
    ) {
        let lines: Vec<ParsedLine<'_>> = hunk.lines.iter().map(|line| parse_line(line)).collect();
        // Counters (R37): both start at the hunk's OLD start.
        let mut old = hunk.old_start;
        let mut new = hunk.old_start;
        let snapshots: Vec<Snapshot> = lines
            .iter()
            .map(|line| {
                let snapshot = Snapshot { old, new };
                match line.kind {
                    Kind::Context => {
                        old += 1;
                        new += 1;
                    }
                    Kind::Removal => old += 1,
                    Kind::Addition => new += 1,
                }
                snapshot
            })
            .collect();

        // Pairs: each maximal run of removals with the maximal run of additions immediately
        // following it, position by position up to the shorter run (R31).
        let mut partner: Vec<Option<usize>> = vec![None; lines.len()];
        let mut index = 0;
        while index < lines.len() {
            if lines[index].kind != Kind::Removal {
                index += 1;
                continue;
            }
            let removal_start = index;
            while index < lines.len() && lines[index].kind == Kind::Removal {
                index += 1;
            }
            let addition_start = index;
            while index < lines.len() && lines[index].kind == Kind::Addition {
                index += 1;
            }
            for offset in 0..(addition_start - removal_start).min(index - addition_start) {
                partner[removal_start + offset] = Some(addition_start + offset);
                partner[addition_start + offset] = Some(removal_start + offset);
            }
        }

        // Word-diff tokens of pairs with similarity >= 0.5 (R31).
        let mut word_tokens: HashMap<usize, Vec<Token>> = HashMap::new();
        for (removal, partner_index) in partner.iter().enumerate() {
            let Some(addition) = *partner_index else {
                continue;
            };
            if lines[removal].kind != Kind::Removal {
                continue;
            }
            let (old_text, new_text) = (lines[removal].text, lines[addition].text);
            if similarity(old_text, new_text) < 0.5 {
                continue;
            }
            let mut removal_tokens = Vec::new();
            let mut addition_tokens = Vec::new();
            for part in diff_words(old_text, new_text) {
                match part.kind {
                    PartKind::Common => {
                        for tokens in [&mut removal_tokens, &mut addition_tokens] {
                            tokens.push(Token {
                                text: part.text.clone(),
                                syntax: None,
                                word: None,
                            });
                        }
                    }
                    PartKind::Removed => removal_tokens.push(Token {
                        text: part.text,
                        syntax: None,
                        word: Some(WordChange::Removed),
                    }),
                    PartKind::Added => addition_tokens.push(Token {
                        text: part.text,
                        syntax: None,
                        word: Some(WordChange::Added),
                    }),
                }
            }
            word_tokens.insert(removal, removal_tokens);
            word_tokens.insert(addition, addition_tokens);
        }

        // Syntax along the before stream (removals and context) and the after stream
        // (additions and context) (R32).
        let before: Vec<usize> = (0..lines.len())
            .filter(|&i| lines[i].kind != Kind::Addition)
            .collect();
        let after: Vec<usize> = (0..lines.len())
            .filter(|&i| lines[i].kind != Kind::Removal)
            .collect();
        let before_spans = stream_spans(
            cache,
            language,
            &before.iter().map(|&i| lines[i].text).collect::<Vec<_>>(),
        );
        let after_spans = stream_spans(
            cache,
            language,
            &after.iter().map(|&i| lines[i].text).collect::<Vec<_>>(),
        );
        let before_position: HashMap<usize, usize> =
            before.iter().enumerate().map(|(p, &i)| (i, p)).collect();
        let after_position: HashMap<usize, usize> =
            after.iter().enumerate().map(|(p, &i)| (i, p)).collect();

        // Context shows the before stream's colors on the left and the after stream's on the
        // right; the one-column stream shows the after stream's.
        let tokens_of = |line: usize, use_before: bool| -> Vec<Token> {
            if let Some(tokens) = word_tokens.get(&line) {
                return tokens.clone();
            }
            let spans = if use_before {
                before_position.get(&line).map(|&p| &before_spans[p])
            } else {
                after_position.get(&line).map(|&p| &after_spans[p])
            };
            match spans {
                Some(spans) if !spans.is_empty() => syntax_tokens(lines[line].text, spans),
                _ => plain_tokens(lines[line].text),
            }
        };
        let kind_of = |line: usize| match lines[line].kind {
            Kind::Context => CellKind::Context,
            Kind::Removal => CellKind::Removal,
            Kind::Addition => CellKind::Addition,
        };

        if !split {
            for line in 0..lines.len() {
                items.push(CompareItem::Unified(Cell {
                    kind: kind_of(line),
                    counter: snapshots[line].new,
                    tokens: tokens_of(line, lines[line].kind == Kind::Removal),
                }));
            }
            return;
        }
        for line in 0..lines.len() {
            match lines[line].kind {
                Kind::Context => items.push(CompareItem::Split {
                    left: Some(Cell {
                        kind: CellKind::Context,
                        counter: snapshots[line].old,
                        tokens: tokens_of(line, true),
                    }),
                    right: Some(Cell {
                        kind: CellKind::Context,
                        counter: snapshots[line].new,
                        tokens: tokens_of(line, false),
                    }),
                }),
                Kind::Removal => {
                    let right = partner[line].map(|addition| Cell {
                        kind: CellKind::Addition,
                        counter: snapshots[addition].new,
                        tokens: tokens_of(addition, false),
                    });
                    items.push(CompareItem::Split {
                        left: Some(Cell {
                            kind: CellKind::Removal,
                            counter: snapshots[line].old,
                            tokens: tokens_of(line, true),
                        }),
                        right,
                    });
                }
                Kind::Addition => {
                    if partner[line].is_some() {
                        continue;
                    }
                    items.push(CompareItem::Split {
                        left: None,
                        right: Some(Cell {
                            kind: CellKind::Addition,
                            counter: snapshots[line].new,
                            tokens: tokens_of(line, false),
                        }),
                    });
                }
            }
        }
    }
}

/// A gutter text (R33, R35): one space, the number left-padded with spaces to `width`, one
/// space. In the one-column stream a counter value of 0 shows blanks (`blank_zero`).
pub fn gutter_text(counter: u32, width: usize, blank_zero: bool) -> String {
    if blank_zero && counter == 0 {
        " ".repeat(width + 2)
    } else {
        format!(" {counter:>width$} ")
    }
}

fn color(r: u8, g: u8, b: u8, a: f32) -> Hsla {
    Rgba {
        r: f32::from(r) / 255.,
        g: f32::from(g) / 255.,
        b: f32::from(b) / 255.,
        a,
    }
    .into()
}

/// The literal colors of the composed viewer (R35, Parameters).
pub mod colors {
    use super::{Hsla, color};

    /// Context body fill `#0f0f0f`.
    pub fn context_body() -> Hsla {
        color(0x0f, 0x0f, 0x0f, 1.)
    }
    /// Addition body fill `rgba(100,250,120,0.047)`.
    pub fn addition_body() -> Hsla {
        color(100, 250, 120, 0.047)
    }
    /// Removal body fill `rgba(255,0,0,0.125)`.
    pub fn removal_body() -> Hsla {
        color(255, 0, 0, 0.125)
    }
    /// Context gutter fill `#050505`.
    pub fn context_gutter() -> Hsla {
        color(5, 5, 5, 1.)
    }
    /// Removal gutter fill `#3c0000`.
    pub fn removal_gutter() -> Hsla {
        color(0x3c, 0, 0, 1.)
    }
    /// Addition gutter fill `#003200`.
    pub fn addition_gutter() -> Hsla {
        color(0, 0x32, 0, 1.)
    }
    /// Gutter text of changed lines: white.
    pub fn changed_number() -> Hsla {
        color(255, 255, 255, 1.)
    }
    /// Gutter text of context lines, the separator and "No changes": `#6c7086`.
    pub fn muted_number() -> Hsla {
        color(0x6c, 0x70, 0x86, 1.)
    }
    /// Plain glyph color `#e6edf3`.
    pub fn plain_glyph() -> Hsla {
        color(0xe6, 0xed, 0xf3, 1.)
    }
    /// Word-level removal background `rgba(255,50,50,0.39)`.
    pub fn word_removal() -> Hsla {
        color(255, 50, 50, 0.39)
    }
    /// Word-level addition background `rgba(0,200,0,0.39)`.
    pub fn word_addition() -> Hsla {
        color(0, 200, 0, 0.39)
    }
}

/// The composed comparison viewer (R30). It paints no surface of its own; size and position
/// come from its [`Styled`] methods and the host supplies scrolling.
#[derive(IntoElement)]
pub struct ComparisonView {
    hunks: Vec<ComparisonHunk>,
    file_path: Option<String>,
    split_view: bool,
    font_family: SharedString,
    font_size: Pixels,
    line_height: Pixels,
    syntax: SyntaxColors,
    cache: Arc<HighlightCache>,
    style: StyleRefinement,
}

impl ComparisonView {
    /// A viewer of `hunks`: one column, no file path, 13 px text on a 20 px line in the default
    /// monospace family.
    pub fn new(hunks: Vec<ComparisonHunk>) -> Self {
        Self {
            hunks,
            file_path: None,
            split_view: false,
            font_family: SharedString::from(vespertine_theme::document::DEFAULT_FONT_MONO),
            font_size: px(13.),
            line_height: px(20.),
            syntax: SyntaxColors::dark().with_plain(colors::plain_glyph()),
            cache: HighlightCache::global(),
            style: StyleRefinement::default(),
        }
    }

    /// The file path whose extension picks the language (R32).
    pub fn file_path(mut self, path: impl Into<String>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    /// Aligned before/after columns instead of one column (default false).
    pub fn split_view(mut self, split: bool) -> Self {
        self.split_view = split;
        self
    }

    /// The monospace family override.
    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = family.into();
        self
    }

    /// The base text size (default 13).
    pub fn font_size(mut self, size: impl Into<Pixels>) -> Self {
        self.font_size = size.into();
        self
    }

    /// The line height (default 20).
    pub fn line_height(mut self, height: impl Into<Pixels>) -> Self {
        self.line_height = height.into();
        self
    }

    /// The syntax palette (colors only).
    pub fn syntax_colors(mut self, colors: SyntaxColors) -> Self {
        self.syntax = colors.with_plain(self::colors::plain_glyph());
        self
    }

    /// The highlight cache to use instead of the global one.
    pub fn cache(mut self, cache: Arc<HighlightCache>) -> Self {
        self.cache = cache;
        self
    }

    /// The processed model of the current properties (what the viewer will show).
    pub fn model(&self) -> CompareModel {
        CompareModel::build(
            &self.hunks,
            self.file_path.as_deref(),
            self.split_view,
            &self.cache,
        )
    }

    fn token_element(&self, token: &Token) -> Div {
        let (text_color, background) = match token.word {
            Some(WordChange::Removed) => (colors::plain_glyph(), Some(colors::word_removal())),
            Some(WordChange::Added) => (colors::plain_glyph(), Some(colors::word_addition())),
            None => (
                token
                    .syntax
                    .map_or_else(colors::plain_glyph, |kind| self.syntax.color_for(kind)),
                None,
            ),
        };
        let element = div()
            .flex_none()
            .whitespace_nowrap()
            .text_color(text_color)
            .child(SharedString::from(token.text.clone()));
        match background {
            Some(background) => element.bg(background),
            None => element,
        }
    }

    fn cell_element(&self, cell: Option<&Cell>, width: usize, blank_zero: bool) -> Div {
        let (gutter_bg, body_bg, number_color, text) = match cell {
            Some(cell) => {
                let (gutter, body, number) = match cell.kind {
                    CellKind::Context => (
                        colors::context_gutter(),
                        colors::context_body(),
                        colors::muted_number(),
                    ),
                    CellKind::Removal => (
                        colors::removal_gutter(),
                        colors::removal_body(),
                        colors::changed_number(),
                    ),
                    CellKind::Addition => (
                        colors::addition_gutter(),
                        colors::addition_body(),
                        colors::changed_number(),
                    ),
                };
                (
                    Some(gutter),
                    Some(body),
                    number,
                    gutter_text(cell.counter, width, blank_zero),
                )
            }
            None => (
                None,
                None,
                colors::muted_number(),
                gutter_text(0, width, true),
            ),
        };
        let gutter = div()
            .flex_none()
            .min_h(self.line_height)
            .whitespace_nowrap()
            .text_color(number_color)
            .child(SharedString::from(text));
        let gutter = match gutter_bg {
            Some(background) => gutter.bg(background),
            None => gutter,
        };
        let tokens = cell.map(|cell| cell.tokens.as_slice()).unwrap_or(&[]);
        let body = div()
            .flex_1()
            .min_w_0()
            .min_h(self.line_height)
            .pl(px(4.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .children(tokens.iter().map(|token| self.token_element(token))),
            );
        let body = match body_bg {
            Some(background) => body.bg(background),
            None => body,
        };
        div().flex().flex_row().child(gutter).child(body)
    }
}

impl Styled for ComparisonView {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ComparisonView {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut root = div()
            .flex()
            .flex_col()
            .font_family(self.font_family.clone())
            .text_size(self.font_size)
            .line_height(self.line_height)
            .text_color(colors::plain_glyph());
        root.style().refine(&self.style);
        if self.hunks.iter().all(|hunk| hunk.lines.is_empty()) {
            return root.child(
                div()
                    .p(px(16.))
                    .text_color(colors::muted_number())
                    .child("No changes"),
            );
        }
        let model = self.model();
        let unified_width = model.left_width.max(model.right_width);
        let mut rows = Vec::with_capacity(model.items.len());
        for item in &model.items {
            rows.push(match item {
                CompareItem::Separator(text) => div()
                    .pl(px(4.))
                    .whitespace_nowrap()
                    .text_color(colors::muted_number())
                    .child(SharedString::from(text.clone())),
                CompareItem::Unified(cell) => {
                    div().child(self.cell_element(Some(cell), unified_width, true))
                }
                CompareItem::Split { left, right } => div()
                    .flex()
                    .flex_row()
                    .child(
                        self.cell_element(left.as_ref(), model.left_width, false)
                            .w_1_2(),
                    )
                    .child(
                        self.cell_element(right.as_ref(), model.right_width, false)
                            .w_1_2(),
                    ),
            });
        }
        root.children(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(old_start: u32, new_start: u32, lines: &[&str]) -> ComparisonHunk {
        ComparisonHunk {
            old_start,
            old_count: lines.len() as u32,
            new_start,
            new_count: lines.len() as u32,
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        }
    }

    fn model(hunks: &[ComparisonHunk], split: bool) -> CompareModel {
        CompareModel::build(hunks, None, split, &HighlightCache::new())
    }

    fn words(cell: &Cell) -> Vec<(&str, Option<WordChange>)> {
        cell.tokens
            .iter()
            .map(|token| (token.text.as_str(), token.word))
            .collect()
    }

    #[test]
    fn similarity_just_below_and_at_one_half() {
        // "abcd" vs "abxy": distance 2 of 4 -> exactly 0.5, inclusive.
        assert_eq!(similarity("abcd", "abxy"), 0.5);
        // "abcde" vs "abxyz": distance 3 of 5 -> 0.4.
        assert!((similarity("abcde", "abxyz") - 0.4).abs() < 1e-12);
        assert_eq!(similarity("", ""), 1.);
        assert_eq!(similarity("", "abc"), 0.);
        assert_eq!(similarity("same old", "same new"), 0.625);
        // UTF-16 code units: an astral character is two units.
        assert_eq!(similarity("\u{1f600}", "\u{1f601}"), 0.5);
    }

    #[test]
    fn pair_at_the_threshold_gets_word_washes_and_below_keeps_syntax() {
        let at = model(&[hunk(1, 1, &["-abcd", "+abxy"])], false);
        let CompareItem::Unified(removal) = &at.items[0] else {
            panic!()
        };
        assert!(
            removal
                .tokens
                .iter()
                .any(|t| t.word == Some(WordChange::Removed))
        );
        let below = model(&[hunk(1, 1, &["-abcde", "+abxyz"])], false);
        let CompareItem::Unified(removal) = &below.items[0] else {
            panic!()
        };
        assert!(removal.tokens.iter().all(|t| t.word.is_none()));
    }

    #[test]
    fn unequal_change_groups_pair_by_position_and_surplus_stays_unpaired() {
        // 2 removals followed by 3 additions: positions 0 and 1 pair, the third addition is
        // unpaired (keeps syntax colors, no word tokens).
        let m = model(
            &[hunk(
                1,
                1,
                &["-one old", "-two old", "+one new", "+two new", "+three"],
            )],
            true,
        );
        assert_eq!(m.items.len(), 3);
        let CompareItem::Split { left, right } = &m.items[0] else {
            panic!()
        };
        assert_eq!(
            words(left.as_ref().unwrap()),
            [("one ", None), ("old", Some(WordChange::Removed))]
        );
        assert_eq!(
            words(right.as_ref().unwrap()),
            [("one ", None), ("new", Some(WordChange::Added))]
        );
        let CompareItem::Split { left, right } = &m.items[2] else {
            panic!()
        };
        assert!(left.is_none());
        let surplus = right.as_ref().unwrap();
        assert!(surplus.tokens.iter().all(|t| t.word.is_none()));
        assert_eq!(surplus.kind, CellKind::Addition);
        // The surplus row sits at its own position after the paired rows (R34).
        let CompareItem::Split { right, .. } = &m.items[1] else {
            panic!()
        };
        assert_eq!(right.as_ref().unwrap().counter, 2);
    }

    #[test]
    fn a_hunk_with_only_removals_has_no_word_washes() {
        let m = model(&[hunk(1, 1, &["-a b", "-c d"])], false);
        for item in &m.items {
            let CompareItem::Unified(cell) = item else {
                panic!()
            };
            assert!(cell.tokens.iter().all(|t| t.word.is_none()));
        }
    }

    #[test]
    fn grammar_state_advances_independently_along_the_before_and_after_streams() {
        let hunks = [hunk(
            1,
            1,
            &[
                " let a = 1;",
                "-/* gone",
                "+let b = 2;",
                " still */ let c = 3;",
            ],
        )];
        let cache = HighlightCache::new();
        let m = CompareModel::build(&hunks, Some("x.js"), false, &cache);
        let comment = |cell: &Cell| {
            cell.tokens
                .iter()
                .any(|t| t.syntax == Some(TokenKind::Comment))
        };
        let cells: Vec<&Cell> = m
            .items
            .iter()
            .map(|item| match item {
                CompareItem::Unified(cell) => cell,
                _ => panic!(),
            })
            .collect();
        assert!(
            comment(cells[1]),
            "the removal opens a comment in the before stream"
        );
        // The addition (after stream) is not inside that comment: it is code.
        assert!(!comment(cells[2]));
        assert!(
            cells[2]
                .tokens
                .iter()
                .any(|t| t.syntax == Some(TokenKind::Keyword))
        );
        // The context line is shown with the after stream's colors in the one column: no
        // comment there, since the comment never opened in the after stream.
        let split = CompareModel::build(&hunks, Some("x.js"), true, &cache);
        let CompareItem::Split { left, right } = &split.items[2] else {
            panic!("{:?}", split.items)
        };
        assert!(
            comment(left.as_ref().unwrap()),
            "before stream: the context line still sits in the comment"
        );
        assert!(
            !comment(right.as_ref().unwrap()),
            "after stream: it is code"
        );
    }

    #[test]
    fn languages_by_extension_and_javascript_fallback() {
        assert_eq!(
            language_for_path(Some("a/b.ts")),
            Some(Language::TypeScript)
        );
        assert_eq!(language_for_path(Some("x.TSX")), Some(Language::Tsx));
        assert_eq!(language_for_path(Some("x.mjs")), Some(Language::JavaScript));
        assert_eq!(language_for_path(Some("x.rs")), Some(Language::Rust));
        assert_eq!(language_for_path(Some("x.yml")), Some(Language::Yaml));
        assert_eq!(
            language_for_path(Some("x.unknown")),
            Some(Language::JavaScript)
        );
        assert_eq!(
            language_for_path(Some("Makefile")),
            Some(Language::JavaScript)
        );
        assert_eq!(language_for_path(None), Some(Language::JavaScript));
        assert_eq!(language_for_path(Some("A.java")), None);
    }

    #[test]
    fn gutter_widths_have_a_two_digit_minimum_and_follow_the_first_hunks_start() {
        let small = model(&[hunk(1, 1, &["-a", "+b", " c"])], false);
        assert_eq!((small.left_width, small.right_width), (2, 2));
        // old total = 1 + (removal + context) = 3; new total = 1 + 2 = 3.
        let wide = model(&[hunk(98, 98, &["-a", "+b", " c"])], false);
        // old total = 98 + 2 = 100 -> 3 digits; new total = 98 + 2 = 100 -> 3 digits.
        assert_eq!((wide.left_width, wide.right_width), (3, 3));
        let uneven = model(&[hunk(97, 1, &["-a", " c", " d"])], false);
        assert_eq!(uneven.left_width, 3);
        assert_eq!(uneven.right_width, 2);
        // A zero start counts as 1.
        let zero = model(&[hunk(0, 0, &[" a"])], false);
        assert_eq!((zero.left_width, zero.right_width), (2, 2));
        assert_eq!(gutter_text(1, 2, false), "  1 ");
        assert_eq!(gutter_text(12, 2, false), " 12 ");
        assert_eq!(gutter_text(0, 2, true), "    ");
        assert_eq!(gutter_text(0, 2, false), "  0 ");
    }

    #[test]
    fn separators_appear_between_hunks() {
        let m = model(
            &[
                hunk(1, 1, &[" a"]),
                hunk(10, 10, &[" b"]),
                hunk(20, 20, &[" c"]),
            ],
            false,
        );
        let separators: Vec<_> = m
            .items
            .iter()
            .filter_map(|item| match item {
                CompareItem::Separator(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(separators, ["    ...", "    ..."]);
        assert!(matches!(m.items[1], CompareItem::Separator(_)));
        assert_eq!(m.items.len(), 5);
        let wide = model(
            &[hunk(1000, 1000, &[" a"]), hunk(2000, 2000, &[" b"])],
            false,
        );
        let CompareItem::Separator(text) = &wide.items[1] else {
            panic!()
        };
        assert_eq!(text, &format!("{}...", " ".repeat(4 + 2)));
    }

    #[test]
    fn split_rows_follow_patch_order_with_one_sided_cells() {
        let m = model(&[hunk(1, 1, &["-a", "-b", "+c", " d", "+e"])], true);
        let shape: Vec<(Option<CellKind>, Option<CellKind>)> = m
            .items
            .iter()
            .map(|item| match item {
                CompareItem::Split { left, right } => (
                    left.as_ref().map(|c| c.kind),
                    right.as_ref().map(|c| c.kind),
                ),
                _ => panic!(),
            })
            .collect();
        use CellKind::*;
        assert_eq!(
            shape,
            [
                (Some(Removal), Some(Addition)),
                (Some(Removal), None),
                (Some(Context), Some(Context)),
                (None, Some(Addition)),
            ]
        );
    }

    #[test]
    fn counters_start_at_the_old_start_for_both_sides_the_preserved_quirk() {
        // Regression fixture for R37: new start 11 differs from old start 1, yet both counters
        // start at 1; displayed new-side numbers are offset by the difference.
        let hunks = [hunk(1, 11, &[" ctx", "-gone", "+came", " tail"])];
        let split = model(&hunks, true);
        let counters: Vec<(Option<u32>, Option<u32>)> = split
            .items
            .iter()
            .map(|item| match item {
                CompareItem::Split { left, right } => (
                    left.as_ref().map(|c| c.counter),
                    right.as_ref().map(|c| c.counter),
                ),
                _ => panic!(),
            })
            .collect();
        // ctx: old 1 / new 1; removal: old 2 paired with the addition's new 2; tail: 3 / 3.
        assert_eq!(
            counters,
            [(Some(1), Some(1)), (Some(2), Some(2)), (Some(3), Some(3))]
        );
        let unified = model(&hunks, false);
        let numbers: Vec<u32> = unified
            .items
            .iter()
            .map(|item| match item {
                CompareItem::Unified(cell) => cell.counter,
                _ => panic!(),
            })
            .collect();
        // The removal shows the current (non-advancing) new counter 2; the addition shows 2
        // and advances.
        assert_eq!(numbers, [1, 2, 2, 3]);
    }

    #[test]
    fn a_removal_shows_the_non_advancing_new_counter_and_additions_their_own_value() {
        let hunks = [hunk(5, 5, &["-a", "-b", "+c"])];
        let unified = model(&hunks, false);
        let numbers: Vec<u32> = unified
            .items
            .iter()
            .map(|item| match item {
                CompareItem::Unified(cell) => cell.counter,
                _ => panic!(),
            })
            .collect();
        assert_eq!(numbers, [5, 5, 5]);
        let split = model(&hunks, true);
        let CompareItem::Split { left, right } = &split.items[0] else {
            panic!()
        };
        assert_eq!(left.as_ref().unwrap().counter, 5);
        assert_eq!(right.as_ref().unwrap().counter, 5);
        let CompareItem::Split { left, right } = &split.items[1] else {
            panic!()
        };
        assert_eq!(left.as_ref().unwrap().counter, 6);
        assert!(right.is_none());
    }

    #[test]
    fn counter_zero_is_blank_in_unified_and_a_padded_zero_in_split() {
        let hunks = [hunk(0, 1, &["+first", " second"])];
        let unified = model(&hunks, false);
        let CompareItem::Unified(cell) = &unified.items[0] else {
            panic!()
        };
        assert_eq!(cell.counter, 0);
        let width = unified.left_width.max(unified.right_width);
        assert_eq!(gutter_text(cell.counter, width, true), "    ");
        let split = model(&hunks, true);
        let CompareItem::Split { right, .. } = &split.items[0] else {
            panic!()
        };
        let cell = right.as_ref().unwrap();
        assert_eq!(cell.counter, 0);
        assert_eq!(gutter_text(cell.counter, split.right_width, false), "  0 ");
    }

    #[test]
    fn the_fidelity_hunk_pairs_with_word_washes() {
        let m = model(
            &[hunk(1, 1, &["-same old", "+same new", " unchanged"])],
            true,
        );
        assert_eq!((m.left_width, m.right_width), (2, 2));
        let CompareItem::Split { left, right } = &m.items[0] else {
            panic!()
        };
        assert_eq!(
            words(left.as_ref().unwrap()),
            [("same ", None), ("old", Some(WordChange::Removed))]
        );
        assert_eq!(
            words(right.as_ref().unwrap()),
            [("same ", None), ("new", Some(WordChange::Added))]
        );
        assert_eq!(left.as_ref().unwrap().counter, 1);
        assert_eq!(right.as_ref().unwrap().counter, 1);
        let CompareItem::Split { left, right } = &m.items[1] else {
            panic!()
        };
        assert_eq!(left.as_ref().unwrap().counter, 2);
        assert_eq!(right.as_ref().unwrap().counter, 2);
        assert_eq!(left.as_ref().unwrap().kind, CellKind::Context);
    }
}
