//! Row rendering of the diff viewer (CMP-015 R9-R12, R21, R22, R25-R28).
//!
//! Code lines are `RichText` runs (`.native()`, selectable and searchable); everything else —
//! gutters, markers, headers, notices, the show-more label — is chrome and never part of a
//! selection or a search.

use super::model::{DiffLine, LineKind, file_notices};
use super::rows::{
    DRAFT_HEIGHT, Draft, Row, Side, annotation_anchor, card_height, line_source_ref,
};
use super::state::{DiffViewState, Hover};
use super::view::{AnnotationAdd, DraftEditor, Frame, LineClick};
use std::ops::Range;
use std::rc::Rc;
use vespertine::{
    AnyElement, App, Bounds, Corners, ElementId, Hsla, InteractiveElement, IntoElement,
    MotionClock, ParentElement, Rgba, Stateful, StatefulInteractiveElement, Styled, Window, canvas,
    div, px, rgb,
};
use vespertine_richtext::{RichText, UnderWash, runs_from_spans};
use vespertine_theme::document::DocumentTheme;

/// The theme tokens the rows read, as `Hsla`, plus the derived ink color.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// White on a theme whose background lightness is below 0.5, black otherwise.
    pub ink: Hsla,
    /// Whether the background is dark (ink is white).
    pub dark: bool,
    /// Viewer surface fill.
    pub bg: Hsla,
    /// Border token.
    pub border: Hsla,
    /// Text token.
    pub text: Hsla,
    /// Dim-text token.
    pub text_dim: Hsla,
    /// Faint-text token.
    pub text_faint: Hsla,
    /// Accent token.
    pub accent: Hsla,
    /// Added-line token.
    pub diff_add: Hsla,
    /// Deleted-line token.
    pub diff_del: Hsla,
    /// Hunk-header background token.
    pub diff_hunk_bg: Hsla,
}

fn hsla_of(color: Rgba) -> Hsla {
    color.into()
}

impl Palette {
    /// Resolves the tokens of `theme`.
    pub fn new(theme: &DocumentTheme) -> Self {
        let bg = hsla_of(theme.bg);
        let dark = bg.l < 0.5;
        let ink = if dark {
            hsla_of(rgb(0xffffff))
        } else {
            hsla_of(rgb(0x000000))
        };
        Self {
            ink,
            dark,
            bg,
            border: hsla_of(theme.border),
            text: hsla_of(theme.text),
            text_dim: hsla_of(theme.text_dim),
            text_faint: hsla_of(theme.text_faint),
            accent: hsla_of(theme.accent),
            diff_add: hsla_of(theme.diff_add),
            diff_del: hsla_of(theme.diff_del),
            diff_hunk_bg: hsla_of(theme.diff_hunk_bg),
        }
    }

    /// The ink at an absolute alpha.
    pub fn ink_alpha(&self, alpha: f32) -> Hsla {
        self.ink.alpha(alpha)
    }
}

/// The colors of a code line (R12).
#[derive(Clone, Copy, Debug)]
pub struct LineColors {
    /// Row tint: the token with alpha replaced by 0.055 (none for context).
    pub tint: Option<Hsla>,
    /// Accent lane: the token with alpha x 0.55 (invisible spacer for context).
    pub lane: Option<Hsla>,
    /// Number color of the changed side: token alpha x 0.9.
    pub changed_number: Hsla,
    /// Number color of an unchanged side or a context line: faint-text x 0.8.
    pub plain_number: Hsla,
    /// Marker color: the token at full alpha, or faint-text x 0.5 for context.
    pub marker: Hsla,
}

/// Colors of a line kind (R12).
pub fn line_colors(palette: &Palette, kind: LineKind) -> LineColors {
    let token = match kind {
        LineKind::Added => Some(palette.diff_add),
        LineKind::Deleted => Some(palette.diff_del),
        _ => None,
    };
    LineColors {
        tint: token.map(|token| token.alpha(0.055)),
        lane: token.map(|token| token.opacity(0.55)),
        changed_number: token.map_or(palette.text_faint.opacity(0.8), |t| t.opacity(0.9)),
        plain_number: palette.text_faint.opacity(0.8),
        marker: token.unwrap_or_else(|| palette.text_faint.opacity(0.5)),
    }
}

fn marker_text(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Added => "+",
        LineKind::Deleted => "\u{2212}",
        _ => "\u{b7}",
    }
}

fn chrome(id: impl Into<ElementId>, text: impl Into<std::sync::Arc<str>>) -> RichText {
    RichText::new(id, text).chrome()
}

fn eid(tag: &'static str, ix: usize, sub: usize) -> ElementId {
    ElementId::from((tag, ix * 4096 + sub))
}

/// Renders row `ix` of the flattened list.
pub(super) fn render_row(
    frame: &Rc<Frame>,
    ix: usize,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let row = frame.flat.rows[ix];
    render_row_value(frame, ix, 0, row, window, cx)
}

fn render_row_value(
    frame: &Rc<Frame>,
    ix: usize,
    sub: usize,
    row: Row,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match row {
        Row::FileHeader { file } => {
            let first = ix == 0 && sub == 0;
            file_header(
                frame,
                file,
                ElementId::from(("diff-file", ix)),
                first,
                false,
            )
            .into_any_element()
        }
        Row::Notice { file, index } => notice_row(frame, ix, sub, file, index),
        Row::HunkHeader { file, hunk } => hunk_row(frame, ix, sub, file, hunk),
        Row::Line { file, hunk, line } => line_row(frame, ix, sub, file, hunk, line, cx),
        Row::Split {
            file,
            hunk,
            left,
            right,
        } => split_row(frame, ix, sub, file, hunk, left, right, cx),
        Row::Card { file, annotation } => card_row(frame, ix, sub, file, annotation),
        Row::Draft { file } => draft_row(frame, ix, sub, file, window, cx),
        Row::BottomPad { .. } => div()
            .id(eid("diff-pad", ix, sub))
            .w_full()
            .h(px(frame.metrics.bottom_pad))
            .flex_none()
            .into_any_element(),
        Row::ShowMore { remaining } => show_more_row(frame, ix, remaining),
        Row::StandIn { file, index } => stand_in_row(frame, ix, file, index, window, cx),
    }
}

// --- file header (R9) and the sticky header (R25) ---

fn file_header(
    frame: &Rc<Frame>,
    file: usize,
    id: ElementId,
    first: bool,
    sticky: bool,
) -> Stateful<vespertine::Div> {
    let palette = &frame.palette;
    let model = &frame.model;
    let entry = &model.files[file];
    let mono = frame.props.theme.font_mono.clone();
    let controlled = frame.props.collapsed.is_some();
    let (rest, hover) = if sticky && !frame.props.frosted {
        // An opaque theme paints the flattened fills (composited over the background).
        (
            palette.bg.blend(palette.ink_alpha(0.025)),
            palette.bg.blend(palette.ink_alpha(0.05)),
        )
    } else {
        (palette.ink_alpha(0.025), palette.ink_alpha(0.05))
    };
    let mut header = div()
        .id(id)
        .w_full()
        .h(px(frame.metrics.file_header))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .bg(rest)
        .border_t_1()
        .border_color(palette.ink_alpha(0.04))
        .hover(move |style| style.bg(hover))
        .cursor_pointer()
        .font_family(mono)
        .child(
            crate::layout::flexible_cell()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.))
                .text_color(palette.text_dim)
                .child(chrome(
                    ("diff-path", file + usize::from(sticky) * 1_000_000),
                    entry.path.clone(),
                )),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(palette.diff_add)
                .child(chrome(
                    ("diff-adds", file + usize::from(sticky) * 1_000_000),
                    format!("+{}", entry.additions),
                )),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(palette.diff_del)
                .child(chrome(
                    ("diff-dels", file + usize::from(sticky) * 1_000_000),
                    format!("\u{2212}{}", entry.deletions),
                )),
        );
    if first && let Some(radius) = frame.radius {
        header = header.rounded_t(radius);
    }
    if !controlled || frame.props.on_toggle_file.is_some() {
        let state = frame.state.clone();
        let props = frame.props.clone();
        let path = entry.path.clone();
        header = header.on_click(move |_, window, cx| {
            if let Some(callback) = &props.on_toggle_file {
                callback(path.clone(), window, cx);
            }
            state.update(cx, |state, cx| state.toggle_file(&path, cx));
        });
    }
    header
}

/// The sticky file header (R25): the first visible row's file, pushed up by the next header.
pub(super) fn sticky_header(
    frame: &Rc<Frame>,
    _window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let (file, push) = frame.state.read(cx).sticky()?;
    let height = frame.metrics.file_header;
    let palette = frame.palette;
    let header = file_header(
        frame,
        file,
        ElementId::from(("diff-sticky", file)),
        false,
        true,
    );
    let mut holder = div()
        .absolute()
        .top(px(push))
        .left_0()
        .w_full()
        .h(px(height))
        .overflow_hidden();
    if frame.props.frosted {
        let tint = palette.bg.alpha(if palette.dark { 0.40 } else { 0.85 });
        holder = holder
            .child(
                canvas(
                    |_, _, _| (),
                    |bounds: Bounds<vespertine::Pixels>, _, window, _| {
                        window.paint_backdrop_blur(bounds, Corners::default(), px(16.));
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(div().absolute().size_full().bg(tint))
            .child(header);
    } else {
        // Opaque: an underlay of the background keeps the rows underneath from showing.
        holder = holder
            .child(div().absolute().size_full().bg(palette.bg))
            .child(header);
    }
    Some(holder.into_any_element())
}

// --- notices, hunk headers, show more (R10) ---

fn notice_row(frame: &Rc<Frame>, ix: usize, sub: usize, file: usize, index: usize) -> AnyElement {
    let text = file_notices(&frame.model.files[file])
        .into_iter()
        .nth(index)
        .unwrap_or_default();
    div()
        .id(eid("diff-notice", ix, sub))
        .w_full()
        .h(px(frame.metrics.notice))
        .flex_none()
        .flex()
        .items_center()
        .px(px(16.))
        .text_size(px(11.))
        .text_color(frame.palette.text_faint)
        .child(chrome(eid("diff-notice-text", ix, sub), text))
        .into_any_element()
}

fn hunk_row(frame: &Rc<Frame>, ix: usize, sub: usize, file: usize, hunk: usize) -> AnyElement {
    let header = frame.model.files[file].hunks[hunk].header.clone();
    div()
        .id(eid("diff-hunk", ix, sub))
        .w_full()
        .h(px(frame.metrics.hunk_header))
        .flex_none()
        .flex()
        .items_center()
        .px(px(16.))
        .bg(frame.palette.diff_hunk_bg)
        .font_family(frame.props.theme.font_mono.clone())
        .text_size(px(11.))
        .text_color(frame.palette.text_faint)
        .child(chrome(eid("diff-hunk-text", ix, sub), header))
        .into_any_element()
}

fn show_more_row(frame: &Rc<Frame>, ix: usize, remaining: usize) -> AnyElement {
    let label = if remaining == 1 {
        "Show 1 more line".to_owned()
    } else {
        format!("Show {remaining} more lines")
    };
    let hover = frame.palette.ink_alpha(0.05);
    let mut row = div()
        .id(eid("diff-more", ix, 0))
        .w_full()
        .h(px(frame.metrics.show_more))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(frame.text_size))
        .text_color(frame.palette.text_dim)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(chrome(eid("diff-more-text", ix, 0), label));
    if let Some(callback) = frame.props.on_show_more.clone() {
        row = row.on_click(move |_, window, cx| callback(remaining, window, cx));
    }
    row.into_any_element()
}

// --- code lines ---

/// The code text of a line as an element: syntax colors per span (R14), the word-change wash
/// under the glyphs (R13). Native text: selectable and searchable.
fn code_text(
    frame: &Frame,
    id: ElementId,
    file: usize,
    hunk: usize,
    line_index: usize,
) -> RichText {
    let line = &frame.model.files[file].hunks[hunk].lines[line_index];
    let plain = frame.palette.text.alpha(0.92);
    let spans: Vec<(Range<usize>, Hsla)> = if frame.props.syntax {
        frame
            .model
            .highlights(file)
            .line_spans(line)
            .iter()
            .map(|span| (span.range.clone(), frame.syntax.color_for(span.kind)))
            .collect()
    } else {
        Vec::new()
    };
    let mut text = RichText::new(id, line.text.clone()).native();
    if !line.text.is_empty() {
        text = text.runs(runs_from_spans(&line.text, &spans, plain, &frame.font));
    }
    if frame.props.word_diff
        && let Some(range) = frame.model.word_intervals(file, hunk)[line_index].clone()
    {
        let token = match line.kind {
            LineKind::Added => frame.palette.diff_add,
            _ => frame.palette.diff_del,
        };
        text = text.under_washes(vec![UnderWash {
            range,
            color: token.alpha(0.28),
            radius: px(3.),
            expand_x: px(1.),
            inset_y: px(1.5),
        }]);
    }
    text
}

/// The code area of a line row: a viewport that pans per file when unwrapped (gutters stay), or
/// a wrapping column (R22).
fn code_area(
    frame: &Rc<Frame>,
    file: usize,
    left_padding: f32,
    hscroll: f32,
    text: RichText,
) -> AnyElement {
    let line_height = frame.metrics.line;
    if frame.props.wrap {
        return div()
            .flex_1()
            .min_w_0()
            .pl(px(left_padding))
            .child(text)
            .into_any_element();
    }
    let state = frame.state.clone();
    div()
        .flex_1()
        .min_w_0()
        .h(px(line_height))
        .overflow_hidden()
        .on_scroll_wheel(move |event, window, cx| {
            let delta = event.delta.pixel_delta(px(line_height));
            let horizontal = if delta.x.abs() > delta.y.abs() {
                Some(-f32::from(delta.x))
            } else if event.modifiers.shift && delta.y != px(0.) {
                Some(-f32::from(delta.y))
            } else {
                None
            };
            if let Some(amount) = horizontal {
                state.update(cx, |state, cx| state.pan(file, amount, window, cx));
                cx.stop_propagation();
            }
        })
        .child(
            div()
                .ml(px(-hscroll))
                .pl(px(left_padding))
                .h(px(line_height))
                .whitespace_nowrap()
                .child(text),
        )
        .into_any_element()
}

fn number_cell(
    frame: &Frame,
    id: ElementId,
    width: f32,
    number: Option<u32>,
    color: Hsla,
) -> AnyElement {
    let cell = div()
        .w(px(width))
        .h(px(frame.metrics.line))
        .flex_none()
        .flex()
        .items_center()
        .justify_end()
        .pr(px(8.))
        .font_family(frame.props.theme.font_mono.clone())
        .text_size(px(11.))
        .text_color(color);
    match number {
        Some(number) => cell
            .child(chrome(id, number.to_string()))
            .into_any_element(),
        None => cell.into_any_element(),
    }
}

fn marker_cell(
    frame: &Frame,
    id: ElementId,
    width: f32,
    kind: LineKind,
    color: Hsla,
) -> AnyElement {
    div()
        .w(px(width))
        .h(px(frame.metrics.line))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .font_family(frame.props.theme.font_mono.clone())
        .text_size(px(frame.text_size))
        .text_color(color)
        .child(chrome(id, marker_text(kind)))
        .into_any_element()
}

fn lane(color: Option<Hsla>) -> AnyElement {
    let lane = div().w(px(3.)).flex_none().self_stretch();
    match color {
        Some(color) => lane.bg(color).into_any_element(),
        None => lane.into_any_element(),
    }
}

fn line_click_handler(
    frame: &Rc<Frame>,
    file: usize,
    line: &DiffLine,
) -> Option<impl Fn(&vespertine::ClickEvent, &mut Window, &mut App) + 'static> {
    let callback = frame.props.on_line_click.clone()?;
    let event = LineClick {
        path: frame.model.files[file].path.clone(),
        text: line.text.clone(),
        old: line.old,
        new: line.new,
        source: line_source_ref(line, frame.model.has_new_document(file)),
    };
    Some(
        move |_: &vespertine::ClickEvent, window: &mut Window, cx: &mut App| {
            callback(event.clone(), window, cx)
        },
    )
}

fn annotation_enabled(frame: &Frame) -> bool {
    frame.props.on_add.is_some() && frame.props.draft_editor.is_some()
}

/// The 16 x 16 add affordance (R28), shown while the row is hovered.
fn add_affordance(
    frame: &Rc<Frame>,
    ix: usize,
    sub: usize,
    file: usize,
    side: Side,
    number: u32,
    left: f32,
) -> AnyElement {
    let state = frame.state.clone();
    let path = frame.model.files[file].path.clone();
    let top = (frame.metrics.line - 16.) / 2.;
    div()
        .id(eid("diff-add", ix, sub))
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(16.))
        .h(px(16.))
        .rounded(px(3.))
        .bg(frame.palette.accent)
        .text_color(frame.palette.bg)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(chrome(eid("diff-add-glyph", ix, sub), "+"))
        .on_click(move |_, _window, cx| {
            cx.stop_propagation();
            let draft = Draft {
                path: path.clone(),
                side,
                line: number,
                editing: None,
            };
            state.update(cx, |state, cx| state.open_draft(draft, cx));
        })
        .into_any_element()
}

fn hover_state(frame: &Frame, ix: usize, cx: &App) -> bool {
    frame.state.read(cx).hover == Some(Hover { row: ix })
}

fn with_hover(
    frame: &Rc<Frame>,
    ix: usize,
    row: Stateful<vespertine::Div>,
) -> Stateful<vespertine::Div> {
    let state = frame.state.clone();
    row.on_hover(move |hovered, _window, cx| {
        state.update(cx, |state: &mut DiffViewState, cx| {
            if *hovered {
                state.set_hover(Some(Hover { row: ix }), cx);
            } else if state.hover == Some(Hover { row: ix }) {
                state.set_hover(None, cx);
            }
        });
    })
}

fn meta_row(frame: &Rc<Frame>, ix: usize, sub: usize, text: &str, left: f32) -> AnyElement {
    div()
        .id(eid("diff-meta", ix, sub))
        .w_full()
        .h(px(frame.metrics.line))
        .flex_none()
        .flex()
        .items_center()
        .pl(px(left))
        .text_size(px(10.5))
        .italic()
        .text_color(frame.palette.text_faint)
        .child(chrome(eid("diff-meta-text", ix, sub), text.to_owned()))
        .into_any_element()
}

fn line_row(
    frame: &Rc<Frame>,
    ix: usize,
    sub: usize,
    file: usize,
    hunk: usize,
    line_index: usize,
    cx: &App,
) -> AnyElement {
    let line = &frame.model.files[file].hunks[hunk].lines[line_index];
    let g = frame.model.gutters[file];
    if line.kind == LineKind::Meta {
        return meta_row(frame, ix, sub, &line.text, 3. + 2. * g + 28. + 12.);
    }
    let colors = line_colors(&frame.palette, line.kind);
    let hscroll = frame.state.read(cx).horizontal_offset(file);
    let (old_color, new_color) = match line.kind {
        LineKind::Deleted => (colors.changed_number, colors.plain_number),
        LineKind::Added => (colors.plain_number, colors.changed_number),
        _ => (colors.plain_number, colors.plain_number),
    };
    let wrap = frame.props.wrap;
    let line_height = frame.metrics.line;
    let mut row = div()
        .id(eid("diff-line", ix, sub))
        .relative()
        .w_full()
        .flex_none()
        .flex()
        .flex_row()
        .items_start();
    row = if wrap {
        row.min_h(px(line_height))
    } else {
        row.h(px(line_height))
    };
    if let Some(tint) = colors.tint {
        row = row.bg(tint);
    }
    let text = code_text(frame, eid("diff-code", ix, sub), file, hunk, line_index);
    row = row
        .child(lane(colors.lane))
        .child(number_cell(
            frame,
            eid("diff-old", ix, sub),
            g,
            line.old,
            old_color,
        ))
        .child(number_cell(
            frame,
            eid("diff-new", ix, sub),
            g,
            line.new,
            new_color,
        ))
        .child(marker_cell(
            frame,
            eid("diff-marker", ix, sub),
            28.,
            line.kind,
            colors.marker,
        ))
        .child(code_area(frame, file, 12., hscroll, text));
    if let Some(handler) = line_click_handler(frame, file, line) {
        row = row.on_click(handler);
    }
    if annotation_enabled(frame)
        && sub == 0
        && let Some(anchor) = annotation_anchor(line)
    {
        row = with_hover(frame, ix, row);
        if hover_state(frame, ix, cx) {
            let left = 3. + if anchor.side == Side::Old { 0. } else { g } + (g - 16.) / 2.;
            row = row.child(add_affordance(
                frame,
                ix,
                sub,
                file,
                anchor.side,
                anchor.line,
                left,
            ));
        }
    }
    row.into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn split_half(
    frame: &Rc<Frame>,
    ix: usize,
    sub: usize,
    file: usize,
    hunk: usize,
    line_index: Option<usize>,
    right: bool,
    hscroll: f32,
) -> AnyElement {
    let half = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_row()
        .items_start()
        .overflow_hidden();
    let Some(line_index) = line_index else {
        return half
            .self_stretch()
            .bg(frame.palette.ink_alpha(0.03))
            .into_any_element();
    };
    let line = &frame.model.files[file].hunks[hunk].lines[line_index];
    let g = frame.model.gutters[file];
    let colors = line_colors(&frame.palette, line.kind);
    let number = if right { line.new } else { line.old };
    let number_color = match (line.kind, right) {
        (LineKind::Deleted, false) | (LineKind::Added, true) => colors.changed_number,
        _ => colors.plain_number,
    };
    let tag = if right { 1 } else { 0 };
    let mut half = half.id(eid(
        if right { "diff-half-r" } else { "diff-half-l" },
        ix,
        sub,
    ));
    if let Some(tint) = colors.tint {
        half = half.bg(tint);
    }
    // A click reports the line of the half it lands on (old text on the left, new on the right).
    if let Some(handler) = line_click_handler(frame, file, line) {
        half = half.on_click(handler);
    }
    let text = code_text(
        frame,
        eid(if right { "diff-code-r" } else { "diff-code-l" }, ix, sub),
        file,
        hunk,
        line_index,
    );
    half.child(lane(colors.lane))
        .child(number_cell(
            frame,
            eid(if right { "diff-num-r" } else { "diff-num-l" }, ix, sub),
            g,
            number,
            number_color,
        ))
        .child(marker_cell(
            frame,
            eid(
                if right { "diff-mark-r" } else { "diff-mark-l" },
                ix,
                sub + tag,
            ),
            18.,
            line.kind,
            colors.marker,
        ))
        .child(code_area(frame, file, 6., hscroll, text))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn split_row(
    frame: &Rc<Frame>,
    ix: usize,
    sub: usize,
    file: usize,
    hunk: usize,
    left: Option<usize>,
    right: Option<usize>,
    cx: &App,
) -> AnyElement {
    let lines = &frame.model.files[file].hunks[hunk].lines;
    let g = frame.model.gutters[file];
    let is_meta = |index: Option<usize>| index.is_some_and(|i| lines[i].kind == LineKind::Meta);
    if is_meta(left) || is_meta(right) {
        // A meta row spans both columns (R21); two different markers sit in their own halves.
        let left_line = left.map(|i| &lines[i]);
        let right_line = right.map(|i| &lines[i]);
        return match (left_line, right_line) {
            (Some(l), Some(r)) if left != right => div()
                .id(eid("diff-meta-split", ix, sub))
                .w_full()
                .h(px(frame.metrics.line))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .text_size(px(10.5))
                .italic()
                .text_color(frame.palette.text_faint)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pl(px(3. + g))
                        .child(chrome(eid("diff-meta-l", ix, sub), l.text.clone())),
                )
                .child(
                    div()
                        .w(px(1.))
                        .flex_none()
                        .self_stretch()
                        .bg(frame.palette.ink_alpha(0.06)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pl(px(3. + g))
                        .child(chrome(eid("diff-meta-r", ix, sub), r.text.clone())),
                )
                .into_any_element(),
            (Some(line), _) | (None, Some(line)) => {
                meta_row(frame, ix, sub, &line.text, 2. * (3. + g))
            }
            (None, None) => div().into_any_element(),
        };
    }
    let hscroll = frame.state.read(cx).horizontal_offset(file);
    let line_height = frame.metrics.line;
    let mut row = div()
        .id(eid("diff-split", ix, sub))
        .relative()
        .w_full()
        .flex_none()
        .flex()
        .flex_row();
    row = if frame.props.wrap {
        row.min_h(px(line_height))
    } else {
        row.h(px(line_height))
    };
    row = row
        .child(split_half(frame, ix, sub, file, hunk, left, false, hscroll))
        .child(
            div()
                .w(px(1.))
                .flex_none()
                .self_stretch()
                .bg(frame.palette.ink_alpha(0.06)),
        )
        .child(split_half(frame, ix, sub, file, hunk, right, true, hscroll));
    // The affordance lives in the right column only; the left column is inert (R28).
    if annotation_enabled(frame)
        && sub == 0
        && let Some(anchor) = right
            .and_then(|i| annotation_anchor(&lines[i]))
            .filter(|anchor| anchor.side == Side::New)
    {
        row = with_hover(frame, ix, row);
        if hover_state(frame, ix, cx) {
            let half_width = ((frame.state.read(cx).width - 1.) / 2.).max(0.);
            let left_edge = half_width + 1.;
            let offset = left_edge + 3. + (g - 16.) / 2.;
            row = row.child(add_affordance(
                frame,
                ix,
                sub,
                file,
                Side::New,
                anchor.line,
                offset,
            ));
        }
    }
    row.into_any_element()
}

// --- annotations (R27-R29) ---

fn card_row(
    frame: &Rc<Frame>,
    ix: usize,
    sub: usize,
    file: usize,
    annotation: usize,
) -> AnyElement {
    let note = &frame.props.annotations[annotation];
    let entry = &frame.model.files[file];
    let height = card_height(&note.body);
    // The card header cites the pre-rename path for old-side anchors (R28).
    let cited = match note.side {
        Side::Old => entry.old_path.as_deref().unwrap_or(&entry.path),
        Side::New => &entry.path,
    };
    let title = format!("{cited}:{}", note.line);
    let body_lines = ((height - 20. - 22. - 6.) / 18.).round();
    let palette = &frame.palette;
    let mut actions = div().flex().flex_row().gap(px(8.)).text_size(px(11.));
    if frame.props.on_edit.is_some() && frame.props.draft_editor.is_some() {
        let state = frame.state.clone();
        let draft = Draft {
            path: note.path.clone(),
            side: note.side,
            line: note.line,
            editing: Some(note.id.clone()),
        };
        actions = actions.child(
            div()
                .id(eid("diff-card-edit", ix, sub))
                .text_color(palette.text_dim)
                .cursor_pointer()
                .child(chrome(eid("diff-card-edit-text", ix, sub), "Edit"))
                .on_click(move |_, _window, cx| {
                    state.update(cx, |state, cx| state.open_draft(draft.clone(), cx));
                }),
        );
    }
    if let Some(remove) = frame.props.on_remove.clone() {
        let id = note.id.clone();
        actions = actions.child(
            div()
                .id(eid("diff-card-remove", ix, sub))
                .text_color(palette.text_dim)
                .cursor_pointer()
                .child(chrome(eid("diff-card-remove-text", ix, sub), "Remove"))
                .on_click(move |_, window, cx| remove(id.clone(), window, cx)),
        );
    }
    div()
        .id(eid("diff-card", ix, sub))
        .w_full()
        .h(px(height))
        .flex_none()
        .pt(px(10.))
        .px(px(16.))
        .child(
            div()
                .h(px(22. + 18. * body_lines))
                .overflow_hidden()
                .flex()
                .flex_col()
                .px(px(10.))
                .border_1()
                .border_color(palette.border)
                .rounded(px(6.))
                .bg(palette.ink_alpha(0.025))
                .child(
                    div()
                        .h(px(22.))
                        .flex_none()
                        .justify_between()
                        .items_center()
                        .flex_row()
                        .flex()
                        .child(
                            div()
                                .font_family(frame.props.theme.font_mono.clone())
                                .text_size(px(11.))
                                .text_color(palette.text_faint)
                                .child(chrome(eid("diff-card-title", ix, sub), title)),
                        )
                        .child(actions),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(palette.text)
                        .child(
                            RichText::new(eid("diff-card-body", ix, sub), note.body.clone())
                                .native(),
                        ),
                ),
        )
        .into_any_element()
}

fn draft_row(
    frame: &Rc<Frame>,
    ix: usize,
    sub: usize,
    file: usize,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (draft, focus) = {
        let state = frame.state.read(cx);
        (state.draft.clone(), state.draft_focus.clone())
    };
    let (Some(draft), Some(editor_fn)) = (draft, frame.props.draft_editor.clone()) else {
        return div()
            .id(eid("diff-draft", ix, sub))
            .h(px(DRAFT_HEIGHT))
            .into_any_element();
    };
    let entry = &frame.model.files[file];
    let initial_body = draft
        .editing
        .as_ref()
        .and_then(|id| frame.props.annotations.iter().find(|note| &note.id == id))
        .map(|note| note.body.clone())
        .unwrap_or_default();
    let commit = {
        let state = frame.state.clone();
        let props = frame.props.clone();
        let draft = draft.clone();
        let old_path = entry.old_path.clone();
        Rc::new(move |body: &str, window: &mut Window, cx: &mut App| {
            let body = body.trim();
            if !body.is_empty() {
                match &draft.editing {
                    Some(id) => {
                        if let Some(edit) = &props.on_edit {
                            edit(id, body, window, cx);
                        }
                    }
                    None => {
                        if let Some(add) = &props.on_add {
                            add(
                                AnnotationAdd {
                                    path: draft.path.clone(),
                                    side: draft.side,
                                    line: draft.line,
                                    body: body.to_owned(),
                                    old_path: old_path.clone(),
                                },
                                window,
                                cx,
                            );
                        }
                    }
                }
            }
            state.update(cx, |state, cx| state.cancel_draft(cx));
        })
    };
    let cancel = {
        let state = frame.state.clone();
        Rc::new(move |_window: &mut Window, cx: &mut App| {
            state.update(cx, |state, cx| state.cancel_draft(cx));
        })
    };
    let editor = DraftEditor {
        path: draft.path.clone(),
        side: draft.side,
        line: draft.line,
        editing: draft.editing,
        initial_body,
        focus,
        commit,
        cancel,
    };
    let content = editor_fn(&editor, _window, cx);
    div()
        .id(eid("diff-draft", ix, sub))
        .w_full()
        .h(px(DRAFT_HEIGHT))
        .flex_none()
        .pt(px(10.))
        .pb(px(6.))
        .px(px(16.))
        .overflow_hidden()
        .child(content)
        .into_any_element()
}

// --- fold stand-in (R26) ---

fn stand_in_row(
    frame: &Rc<Frame>,
    ix: usize,
    file: usize,
    index: usize,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let stand_in = &frame.flat.stand_ins[index];
    let now = MotionClock::now_ms(cx);
    let height = {
        let state = frame.state.read(cx);
        match state.tweens.get(&file) {
            Some(tween) => tween.height(now, stand_in.body_height),
            None => stand_in.body_height,
        }
    };
    let rows: Vec<AnyElement> = stand_in
        .rows
        .iter()
        .enumerate()
        .map(|(sub, row)| render_row_value(frame, ix, sub + 1, *row, window, cx))
        .collect();
    div()
        .id(eid("diff-standin", ix, 0))
        .w_full()
        .h(px(height))
        .flex_none()
        .overflow_hidden()
        .child(div().flex().flex_col().w_full().children(rows))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        let mut theme = DocumentTheme::dark();
        theme.diff_add = Rgba {
            r: 0.,
            g: 0.8,
            b: 0.5,
            a: 0.9,
        };
        theme.diff_del = Rgba {
            r: 1.,
            g: 0.3,
            b: 0.3,
            a: 0.8,
        };
        Palette::new(&theme)
    }

    #[test]
    fn changed_rows_use_the_specified_alphas_of_the_diff_tokens() {
        let palette = palette();
        let added = line_colors(&palette, LineKind::Added);
        // The row tint REPLACES the alpha; the lane and the number MULTIPLY it.
        assert!((added.tint.unwrap().a - 0.055).abs() < 1e-6);
        assert!((added.lane.unwrap().a - 0.9 * 0.55).abs() < 1e-6);
        assert!((added.changed_number.a - 0.9 * 0.9).abs() < 1e-6);
        assert_eq!(
            added.marker, palette.diff_add,
            "the marker is at full alpha"
        );
        let deleted = line_colors(&palette, LineKind::Deleted);
        assert!((deleted.tint.unwrap().a - 0.055).abs() < 1e-6);
        assert!((deleted.lane.unwrap().a - 0.8 * 0.55).abs() < 1e-6);
        assert!((deleted.changed_number.a - 0.8 * 0.9).abs() < 1e-6);
        assert_eq!(deleted.marker, palette.diff_del);
        assert_eq!(added.tint.unwrap().h, palette.diff_add.h, "same hue");
    }

    #[test]
    fn context_rows_have_no_tint_or_lane_and_faint_numbers_and_markers() {
        let palette = palette();
        let context = line_colors(&palette, LineKind::Context);
        assert!(context.tint.is_none() && context.lane.is_none());
        assert!((context.changed_number.a - palette.text_faint.a * 0.8).abs() < 1e-6);
        assert!((context.plain_number.a - palette.text_faint.a * 0.8).abs() < 1e-6);
        assert!((context.marker.a - palette.text_faint.a * 0.5).abs() < 1e-6);
        assert_eq!(context.marker.h, palette.text_faint.h);
    }

    #[test]
    fn markers_are_plus_true_minus_and_middle_dot() {
        assert_eq!(marker_text(LineKind::Added), "+");
        assert_eq!(marker_text(LineKind::Deleted), "\u{2212}");
        assert_eq!(marker_text(LineKind::Context), "\u{b7}");
    }

    #[test]
    fn ink_is_white_on_dark_backgrounds_and_black_on_light_ones() {
        let dark = Palette::new(&DocumentTheme::dark());
        assert!(dark.dark && dark.ink.l > 0.99 && dark.ink.a == 1.);
        let light = Palette::new(&DocumentTheme::light());
        assert!(!light.dark && light.ink.l < 0.01);
        assert!((dark.ink_alpha(0.025).a - 0.025).abs() < 1e-6);
    }
}
