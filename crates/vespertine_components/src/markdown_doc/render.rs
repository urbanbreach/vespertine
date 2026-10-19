//! Rendering of the Markdown block tree as native elements (CMP-011 R4-R15, R20-R22).

use super::cache::style_identity;
use super::flatten::{Flat, TextCtx, flatten, single_line};
use super::options::MarkdownOptions;
use super::table::TableGeometry;
use super::tree::{Align, Block, BlockKind, Formula, Item, Run, TaskMarker};
use crate::code::{CardAction, CodeCard, CodeScrollState, CopyOptions, WrapToggle, widgets::Icon};
use crate::diagram::DiagramView;
use crate::doc_theme::{alpha, hsla, ink_alpha};
use crate::math::{DisplayBlock, MathBlock, MathTextOptions, inline_line_height, math_text};
use std::cell::Cell;
use std::rc::Rc;
use vespertine::{
    AnyElement, App, ClickEvent, CursorStyle, ElementId, Font, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Pixels, Role, SharedString,
    StatefulInteractiveElement, Styled, TextRun, Window, div, font, px,
};
use vespertine_richtext::{RichText, RunKey, UnderWash, UserSelect, user_select};
use vespertine_theme::document::DocumentTheme;

/// Render context of one top-level block.
pub struct Ctx<'a> {
    pub opts: &'a MarkdownOptions,
    pub top: usize,
    /// Whether this is the last top-level block (the streaming fade applies to it).
    pub last: bool,
    elem: Cell<usize>,
}

impl<'a> Ctx<'a> {
    /// A context for top-level block `top`.
    pub fn new(opts: &'a MarkdownOptions, top: usize, last: bool) -> Self {
        Self {
            opts,
            top,
            last,
            elem: Cell::new(0),
        }
    }

    fn theme(&self) -> &DocumentTheme {
        &self.opts.theme
    }

    /// Flattens through the document's cache (keyed by block generation and style identity).
    fn flat(&self, runs: &[Run], text: &TextCtx, hide_task_marker: bool) -> Flat {
        let theme = self.theme();
        let math = &self.opts.math;
        let build = || flatten(runs, theme, text, hide_task_marker, math);
        let Some(cache) = &self.opts.cache else {
            return build();
        };
        let generation = self.opts.generations.get(self.top).copied().unwrap_or(0);
        let mut style = style_identity(theme);
        // The text context and marker handling are part of the identity of one flattening.
        let extra = (
            text.size.as_f32().to_bits(),
            text.weight.0.to_bits(),
            hide_task_marker,
            (math.enabled, math.face.index(), math.size()),
        );
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(
            &(
                style,
                extra,
                [
                    text.color.h.to_bits(),
                    text.color.s.to_bits(),
                    text.color.l.to_bits(),
                    text.color.a.to_bits(),
                ],
            ),
            &mut hasher,
        );
        style = std::hash::Hasher::finish(&hasher);
        let element = self.elem.get();
        cache
            .borrow_mut()
            .get_or_insert(self.top, element, generation, style, build)
    }

    /// The next element index of this block.
    fn next(&self) -> usize {
        let elem = self.elem.get();
        self.elem.set(elem + 1);
        elem
    }

    fn name(&self, elem: usize, kind: &str) -> SharedString {
        format!("{}-{}-{}-{kind}", self.opts.row_key, self.top, elem).into()
    }

    fn key(&self, elem: usize, sub: u32) -> RunKey {
        RunKey::new((self.opts.row_key.clone(), self.top, elem), sub)
    }
}

/// Renders blocks stacked with the block gap.
pub fn render_blocks(
    blocks: &[Block],
    base_top: usize,
    opts: &MarkdownOptions,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    let theme = &opts.theme;
    let text = TextCtx::body(theme);
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let ctx = Ctx::new(opts, base_top + index, index + 1 == blocks.len());
            render_block(block, &ctx, &text, window, cx)
        })
        .collect()
}

/// Renders a single block with its top index.
pub fn render_block(
    block: &Block,
    ctx: &Ctx,
    text: &TextCtx,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = ctx.theme();
    match &block.kind {
        BlockKind::Paragraph(runs) => paragraph(runs, ctx, text, window, cx),
        BlockKind::Heading { level, runs } => {
            let (size, line_height) = theme.metrics.heading_tier(i32::from(*level));
            let ctx_text = TextCtx {
                weight: FontWeight::SEMIBOLD,
                size: px(size),
                line_height: px(line_height),
                ..text.clone()
            };
            let flat = ctx.flat(runs, &ctx_text, false);
            text_element(ctx, flat, &ctx_text, None, window, cx)
        }
        BlockKind::Code { info, code, .. } => code_block(info, code, ctx, window, cx),
        BlockKind::Quote(children) => quote(children, ctx, text, window, cx),
        BlockKind::List {
            ordered,
            start,
            items,
        } => list(*ordered, *start, items, ctx, text, window, cx),
        BlockKind::Rule => div()
            .w_full()
            .h(px(1.))
            .flex_none()
            .bg(hsla(theme.border))
            .into_any_element(),
        BlockKind::Table {
            aligns,
            header,
            rows,
        } => table(aligns, header, rows, ctx, text, window, cx),
        BlockKind::DisplayMath(formula) => display_math(formula, ctx, text, window, cx),
    }
}

/// A text element: one native run with the block's runs, links and pills.
fn text_element(
    ctx: &Ctx,
    mut flat: Flat,
    text: &TextCtx,
    align: Option<Align>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let elem = ctx.next();
    if ctx.last
        && let Some(fade) = ctx.opts.fade
    {
        flat.fade_tail(fade);
    }
    let theme = ctx.theme();
    // A tall inline formula opens the line height of every line of its paragraph (CMP-012 R7).
    let line_height = match &flat.math {
        Some(math) => {
            let system = window.text_system().clone();
            let runs = flat
                .runs
                .iter()
                .map(|run| (run.len, run.font.clone()))
                .collect();
            inline_line_height(
                &math.flat(runs),
                math.formulas(),
                ctx.opts.math.scale(),
                text.line_height,
                text.size,
                |font: &Font| {
                    let id = system.resolve_font(font);
                    (
                        f32::from(system.ascent(id, px(100.))) / 100.,
                        f32::from(system.descent(id, px(100.))) / 100.,
                    )
                },
            )
        }
        None => text.line_height,
    };
    let mut element = div()
        .w_full()
        .min_w_0()
        .text_size(text.size)
        .line_height(line_height)
        .text_color(text.color)
        .font_family(theme.font_sans.clone())
        .font_weight(text.weight);
    element = match align {
        Some(Align::Center) => element.flex().flex_row().justify_center().text_center(),
        Some(Align::Right) => element.flex().flex_row().justify_end().text_right(),
        _ => element,
    };
    if flat.is_empty() {
        return element.min_h(text.line_height).into_any_element();
    }
    let rich = match &flat.math {
        Some(math) => math_text(
            ctx.name(elem, "text"),
            math,
            flat.runs.clone(),
            MathTextOptions::inline(ctx.opts.math.scale()),
            window,
            cx,
        ),
        None => RichText::new(ctx.name(elem, "text"), flat.text.clone()).runs(flat.runs.clone()),
    };
    let mut run = rich
        .key(ctx.key(elem, 0))
        .native()
        .links(flat.links.clone())
        .link_options(ctx.opts.link_options.clone());
    if !flat.pills.is_empty() {
        let (color, radius) = (
            hsla(theme.code_wash),
            px(theme.metrics.md_inline_code_radius),
        );
        run = run.under_washes(
            flat.pills
                .into_iter()
                .map(|range| UnderWash {
                    range,
                    color,
                    radius,
                    expand_x: px(2.),
                    inset_y: px(2.),
                })
                .collect(),
        );
    }
    element.child(run).into_any_element()
}

fn paragraph(
    runs: &[Run],
    ctx: &Ctx,
    text: &TextCtx,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = ctx.theme();
    let hide_marker = ctx.opts.task.is_some();
    // Images with an owner resolver split the paragraph around the media element.
    if let Some(resolver) = ctx.opts.media.image.clone()
        && runs.iter().any(|run| run.image.is_some())
    {
        let mut pieces: Vec<AnyElement> = Vec::new();
        let mut pending: Vec<Run> = Vec::new();
        for run in runs {
            if let Some(image) = &run.image {
                let key = format!("{}-{}-image-{}", ctx.opts.row_key, ctx.top, ctx.elem.get());
                if let Some(element) = resolver(image, &key, theme, window, cx) {
                    ctx.next();
                    if !pending.is_empty() {
                        let flat = ctx.flat(&pending, text, hide_marker);
                        pieces.push(text_element(ctx, flat, text, None, window, cx));
                        pending.clear();
                    }
                    pieces.push(element);
                    continue;
                }
            }
            pending.push(run.clone());
        }
        if !pending.is_empty() {
            let flat = ctx.flat(&pending, text, hide_marker);
            pieces.push(text_element(ctx, flat, text, None, window, cx));
        }
        return div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme.metrics.md_block_gap))
            .children(pieces)
            .into_any_element();
    }
    let flat = ctx.flat(runs, text, hide_marker);
    text_element(ctx, flat, text, None, window, cx)
}

fn quote(
    children: &[Block],
    ctx: &Ctx,
    text: &TextCtx,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = ctx.theme();
    let accent = hsla(theme.accent);
    let muted_text = text.with_color(hsla(theme.text_muted));
    let inner: Vec<AnyElement> = children
        .iter()
        .map(|child| render_block(child, ctx, &muted_text, window, cx))
        .collect();
    div()
        .relative()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.))
        .pl(px(14.))
        .pr(px(10.))
        .py(px(6.))
        .bg(alpha(accent, 0.05))
        .rounded_tr(px(6.))
        .rounded_br(px(6.))
        .child(rail(alpha(accent, 0.6)))
        .children(inner)
        .into_any_element()
}

fn list(
    ordered: bool,
    start: u64,
    items: &[Item],
    ctx: &Ctx,
    text: &TextCtx,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = ctx.theme();
    let accent = alpha(hsla(theme.accent), 0.85);
    let rows: Vec<AnyElement> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let task = item_task(item);
            let marker: AnyElement = match (&task, &ctx.opts.task) {
                (Some(marker), Some(options)) => checkbox(marker, item, options, ctx, text),
                _ if ordered => {
                    let elem = ctx.next();
                    div()
                        .flex_none()
                        .min_w(px(18.))
                        .h(text.line_height)
                        .text_size(text.size)
                        .line_height(text.line_height)
                        .text_color(accent)
                        .font_family(theme.font_sans.clone())
                        .child(
                            RichText::new(
                                ctx.name(elem, "marker"),
                                format!("{}.", start + index as u64),
                            )
                            .chrome(),
                        )
                        .into_any_element()
                }
                _ => div()
                    .flex_none()
                    .min_w(px(18.))
                    .h(text.line_height)
                    .flex()
                    .items_center()
                    .child(div().ml(px(1.)).size(px(5.)).rounded_full().bg(accent))
                    .into_any_element(),
            };
            let content: Vec<AnyElement> = item
                .blocks
                .iter()
                .map(|block| render_block(block, ctx, text, window, cx))
                .collect();
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(8.))
                .child(marker)
                .child(item_content(content))
                .into_any_element()
        })
        .collect();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(4.))
        .children(rows)
        .into_any_element()
}

/// The task marker of an item: the marker run at the start of its first paragraph.
fn item_task(item: &Item) -> Option<TaskMarker> {
    match item.blocks.first().map(|block| &block.kind) {
        Some(BlockKind::Paragraph(runs)) => runs.first().and_then(|run| run.task.clone()),
        _ => None,
    }
}

fn checkbox(
    marker: &TaskMarker,
    item: &Item,
    options: &super::options::TaskOptions,
    ctx: &Ctx,
    text: &TextCtx,
) -> AnyElement {
    let theme = ctx.theme();
    let elem = ctx.next();
    let accent = hsla(theme.accent);
    let label: String = match item.blocks.first().map(|block| &block.kind) {
        Some(BlockKind::Paragraph(runs)) => runs
            .iter()
            .filter(|run| run.task.is_none())
            .map(|run| run.text.as_str())
            .collect(),
        _ => String::new(),
    };
    let checked = marker.checked;
    let mut boxed = div()
        .id(ctx.name(elem, "task"))
        .flex()
        .items_center()
        .justify_center()
        .size(px(16.))
        .rounded(px(3.))
        .border_1()
        .border_color(if checked { accent } else { hsla(theme.border) })
        .bg(if checked { accent } else { alpha(accent, 0.) })
        .role(Role::CheckBox)
        .aria_label(SharedString::from(label))
        .aria_toggled(if checked {
            vespertine::accesskit::Toggled::True
        } else {
            vespertine::accesskit::Toggled::False
        })
        .when(checked, |boxed| {
            boxed.child(crate::code::widgets::icon(
                Icon::Check,
                px(12.),
                hsla(theme.bg),
            ))
        });
    match &options.on_toggle {
        Some(on_toggle) => {
            let on_toggle = on_toggle.clone();
            let marker = marker.clone();
            let focus = hsla(theme.text);
            boxed = boxed
                .cursor(CursorStyle::PointingHand)
                .focus_visible(move |style| style.border_color(focus))
                .on_mouse_down(vespertine::MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation()
                })
                .on_click(move |_: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    on_toggle(&marker, window, cx)
                });
        }
        None => boxed = boxed.opacity(0.5),
    }
    user_select(
        UserSelect::None,
        div()
            .flex_none()
            .min_w(px(18.))
            .h(text.line_height)
            .flex()
            .items_center()
            .child(boxed),
    )
    .into_any_element()
}

use vespertine::prelude::FluentBuilder as _;

fn code_block(info: &str, code: &str, ctx: &Ctx, window: &mut Window, cx: &mut App) -> AnyElement {
    let language = info.split_whitespace().next().unwrap_or("").to_owned();
    let options = ctx.opts;
    let theme = ctx.theme().clone();
    let top = ctx.top;
    let elem = ctx.next();
    let id: SharedString = ctx.name(elem, "code");

    let mermaid = language.eq_ignore_ascii_case("mermaid");
    let diagram = if mermaid {
        options.media.diagram.clone()
    } else {
        None
    };
    let mut view = DiagramView::Source;
    if let Some(handler) = &diagram {
        let frame = format!("{}-mermaid-{}", options.row_key, top);
        view = handler(code, &frame, &theme, window, cx);
    }

    let mut card = CodeCard::new(id, top, code)
        .language(language.clone())
        .theme(theme.clone())
        .key_owner((options.row_key.clone(), top, elem));
    let ui = options.code.ui.get(&top);
    if let Some(ui) = ui
        && let Some(scroll) = &ui.scroll
    {
        card = card.scroll_state(scroll.clone());
    }
    if let Some(on_copy) = &options.code.on_copy {
        card = card.copy(CopyOptions {
            copied_index: options.code.copied_index,
            on_copy: on_copy.clone(),
        });
    }
    if let Some(on_toggle) = &options.code.on_toggle_wrap {
        let on_toggle = on_toggle.clone();
        card = card.wrap_toggle(WrapToggle {
            wrapped: ui.is_some_and(|ui| ui.wrapped),
            on_toggle: Rc::new(move |window, cx| on_toggle(top, window, cx)),
        });
    }
    if let Some(actions) = &options.code.actions {
        for action in actions(top, &language) {
            card = card.action(action);
        }
    }
    if let Some(highlight) = &options.code.highlight {
        card = card.highlight(highlight(top, &language, code));
    }
    card = card.scroll_indicator(true);

    match view {
        DiagramView::Source => card.into_any_element(),
        DiagramView::Failed(reason) => card.notice(reason).into_any_element(),
        DiagramView::Diagram {
            body,
            show_source,
            toggle,
        } => {
            let label = if show_source {
                "Show diagram"
            } else {
                "Show source"
            };
            let card = card.action(CardAction {
                label: label.into(),
                icon: Icon::Wrap,
                active: show_source,
                on_click: toggle,
            });
            if show_source {
                card.into_any_element()
            } else {
                diagram_body(card, body, &theme)
            }
        }
    }
}

/// A rendered diagram in the card chrome (header with its actions, no code body).
fn diagram_body(card: CodeCard, body: AnyElement, theme: &DocumentTheme) -> AnyElement {
    card.body_override(body)
        .theme(theme.clone())
        .into_any_element()
}

/// A display formula (CMP-012 R8, R9) as a block of its own; one that cannot be typeset stays a
/// code block labelled `math`.
fn display_math(
    formula: &Formula,
    ctx: &Ctx,
    text: &TextCtx,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = ctx.theme().clone();
    let elem = ctx.next();
    let run = TextRun {
        len: 0,
        font: font(theme.font_sans.clone()),
        color: text.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let block = DisplayBlock::new(
        ctx.name(elem, "math"),
        &formula.tex,
        ctx.opts.math,
        run,
        text.size,
    )
    .line_height(text.line_height)
    .key(ctx.key(elem, 0))
    .native()
    .build(window, cx);
    match block {
        MathBlock::Typeset(element) => element,
        MathBlock::Fallback { language, text } => {
            CodeCard::new(ctx.name(elem, "math-code"), ctx.top, text)
                .label(language)
                .theme(theme)
                .key_owner((ctx.opts.row_key.clone(), ctx.top, elem))
                .into_any_element()
        }
    }
}

/// Characters a line may break at: whitespace except the no-break spaces that formula slots are
/// made of.
fn breaks_line(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\u{a0}' | '\u{2007}' | '\u{202f}')
}

/// The runs of `runs` restricted to the byte `range` of the text they cover.
fn slice_runs(runs: &[TextRun], range: std::ops::Range<usize>) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut offset = 0;
    for run in runs {
        let (start, end) = (offset.max(range.start), (offset + run.len).min(range.end));
        if start < end {
            out.push(TextRun {
                len: end - start,
                ..run.clone()
            });
        }
        offset += run.len;
    }
    out
}

/// The width of the widest unbreakable word of `flat` that contains a typeset formula: the slot
/// text plus the text glued to it, shaped by `shape`. When that word exceeds 512 bytes the
/// formula's own scaled extent stands in (CMP-011 R14).
fn widest_formula_word(
    flat: &Flat,
    size: Pixels,
    scale: f32,
    shape: &dyn Fn(&str, &[TextRun], Pixels) -> f32,
) -> f32 {
    let Some(math) = &flat.math else {
        return 0.;
    };
    let text = flat.text.as_str();
    let mut widest = 0f32;
    for (slot, formula) in math.slots().iter().zip(math.formulas()) {
        let start = text[..slot.start].rfind(breaks_line).map_or(0, |at| {
            at + text[at..].chars().next().map_or(1, char::len_utf8)
        });
        let end = text[slot.end..]
            .find(breaks_line)
            .map_or(text.len(), |at| slot.end + at);
        let width = if end - start > WORD_CAP {
            formula.extent() * scale * size.as_f32()
        } else {
            shape(&text[start..end], &slice_runs(&flat.runs, start..end), size)
        };
        widest = widest.max(width);
    }
    widest
}

/// Longest word shaped for the formula-column rule; longer words use the formula's own extent.
const WORD_CAP: usize = 512;

#[allow(clippy::too_many_arguments)]
fn table(
    aligns: &[Align],
    header: &[Vec<Run>],
    rows: &[Vec<Vec<Run>>],
    ctx: &Ctx,
    text: &TextCtx,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = ctx.theme();
    let columns = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 || (header.iter().all(|c| c.is_empty()) && rows.is_empty()) {
        return div().into_any_element();
    }
    let metrics = theme.metrics;
    let padding = metrics.md_table_cell_padding;
    let header_text = TextCtx {
        weight: FontWeight::BOLD,
        ..text.clone()
    };
    let hairline = ink_alpha(theme, 0.10);

    // Flatten every cell once; measure each column's widest unwrapped content.
    let header_flat: Vec<Flat> = (0..columns)
        .map(|c| {
            flatten(
                header.get(c).map_or(&[][..], |v| &v[..]),
                theme,
                &header_text,
                false,
                &ctx.opts.math,
            )
        })
        .collect();
    let body_flat: Vec<Vec<Flat>> = rows
        .iter()
        .map(|row| {
            (0..columns)
                .map(|c| {
                    flatten(
                        row.get(c).map_or(&[][..], |v| &v[..]),
                        theme,
                        text,
                        false,
                        &ctx.opts.math,
                    )
                })
                .collect()
        })
        .collect();
    let measure = |flat: &Flat, size: Pixels| -> f32 {
        if flat.is_empty() {
            return 0.;
        }
        let line = window
            .text_system()
            .shape_line(single_line(&flat.text), size, &flat.runs, None);
        line.width.as_f32()
    };
    // A column holding an inline formula is at least as wide as the widest unbreakable word
    // containing the formula (CMP-011 R14).
    let shape_word = |text: &str, runs: &[TextRun], size: Pixels| -> f32 {
        window
            .text_system()
            .shape_line(single_line(text), size, runs, None)
            .width
            .as_f32()
    };
    let scale = ctx.opts.math.scale();
    let mut content = vec![0f32; columns];
    let mut words = vec![0f32; columns];
    let mut visit = |c: usize, flat: &Flat, size: Pixels| {
        content[c] = content[c].max(measure(flat, size));
        words[c] = words[c].max(widest_formula_word(flat, size, scale, &shape_word));
    };
    for (c, flat) in header_flat.iter().enumerate() {
        visit(c, flat, header_text.size);
    }
    for row in &body_flat {
        for (c, flat) in row.iter().enumerate() {
            visit(c, flat, text.size);
        }
    }
    for c in 0..columns {
        content[c] = content[c].max(words[c]);
    }
    let geometry = TableGeometry::from_columns(
        &content,
        &words,
        padding,
        metrics.md_table_min_column_content,
        metrics.md_table_min_column_width,
    );

    let elem = ctx.next();
    let scroll_state = window
        .use_keyed_state(
            (ElementId::from(ctx.name(elem, "table-scroll")), "state"),
            cx,
            |_, _| CodeScrollState::new(),
        )
        .read(cx)
        .clone();

    let cell =
        |flat: Flat, c: usize, text: &TextCtx, window: &mut Window, cx: &mut App| -> AnyElement {
            let align = aligns.get(c).copied().unwrap_or_default();
            let mut cell_style = div()
                .flex_basis(px(0.))
                .min_w(px(geometry.floors[c]))
                .p(px(padding))
                .overflow_hidden();
            cell_style = cell_style
                .flex_grow(geometry.naturals[c])
                .flex_shrink(geometry.naturals[c]);
            cell_style
                .child(text_element(ctx, flat, text, Some(align), window, cx))
                .into_any_element()
        };

    let mut row_elements: Vec<AnyElement> = Vec::new();
    row_elements.push(
        div()
            .flex()
            .flex_row()
            .w_full()
            .children(
                header_flat
                    .into_iter()
                    .enumerate()
                    .map(|(c, flat)| cell(flat, c, &header_text, window, cx)),
            )
            .into_any_element(),
    );
    for row in body_flat {
        row_elements.push(
            div()
                .flex()
                .flex_row()
                .w_full()
                .border_t_1()
                .border_color(hairline)
                .children(
                    row.into_iter()
                        .enumerate()
                        .map(|(c, flat)| cell(flat, c, text, window, cx)),
                )
                .into_any_element(),
        );
    }
    div()
        .id(ctx.name(elem, "table"))
        .w_full()
        .flex()
        .flex_row()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .track_scroll(scroll_state.handle())
        .child(table_body(geometry.min_width, row_elements))
        .into_any_element()
}

/// The vertical stack of the table rows: full width, never shrinking below the columns' floors.
fn table_body(min_width: f32, rows: Vec<AnyElement>) -> impl IntoElement {
    div()
        .w_full()
        .min_w(px(min_width))
        .flex_none()
        .flex()
        .flex_col()
        .children(rows)
}

/// The content column of a list item: its blocks stacked with the 4 px item content gap.
fn item_content(blocks: Vec<AnyElement>) -> impl IntoElement {
    div()
        .gap(px(4.))
        .min_w_0()
        .flex_1()
        .flex()
        .flex_col()
        .children(blocks)
}

/// The 2 px rail along the left edge of a block quote.
fn rail(color: Hsla) -> impl IntoElement {
    div()
        .w(px(2.))
        .bg(color)
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
}
