//! The terminal element (CMP-021 R5-R9, R19): measures metrics, reports the grid geometry to its
//! owner and paints background runs, the selection wash, column-pinned glyphs and the cursor.

use vespertine::{
    App, BorderStyle, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId, Entity,
    Font, FontFeatures, FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Rgba, ScrollWheelEvent, ShapedLine, SharedString, Style, TextAlign, TextRun,
    UnderlineStyle, Window, fill, outline, point, px, relative, size,
};

use super::emulator::{CellColor, CellSnapshot, CursorSnapshot};
use super::geometry::{FALLBACK_CELL_WIDTH_RATIO, PADDING, grid_dimensions, scrollbar_metrics};
use super::panel::{GridGeometry, TerminalPanel};
use super::theme::{CURSOR_BLOCK_ALPHA, DIM_ALPHA, TerminalTheme};

/// Font features of the terminal font (R6): ligature and contextual substitutions are disabled
/// (`liga`, `calt`, `dlig` set to 0), because contextual substitution collapses several cells
/// into fewer glyphs and the row would render short while the cursor and backgrounds stay on
/// the true column.
pub fn terminal_font_features() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![
        ("liga".to_string(), 0),
        ("calt".to_string(), 0),
        ("dlig".to_string(), 0),
    ]))
}

/// The terminal font of a theme: its family with ligatures disabled.
pub fn terminal_font(theme: &TerminalTheme) -> Font {
    Font {
        family: theme.font_family.clone(),
        features: terminal_font_features(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// A glyph segment starting at a known grid column (R8).
#[derive(Clone, Debug)]
pub struct Segment {
    /// The grid column the segment starts at.
    pub col: usize,
    /// The text to shape.
    pub text: String,
    /// Style runs covering `text`.
    pub runs: Vec<TextRun>,
    /// Whether any cell of the segment has an underline.
    pub underlined: bool,
}

/// Splits a row into column-pinned segments (R8): consecutive ASCII, non-wide cells without
/// combining marks shape together as one line; every non-ASCII character, wide cell or cell with
/// combining marks is its own segment pinned at its own column; wide-spacer cells are skipped;
/// hidden cells render as a space. Segments of blanks without an underline are dropped.
pub fn row_segments(cells: &[CellSnapshot], theme: &TerminalTheme, base: &Font) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut current: Option<(Segment, usize)> = None;
    let flush = |segments: &mut Vec<Segment>, current: &mut Option<(Segment, usize)>| {
        if let Some((mut segment, _)) = current.take() {
            trim_trailing_blanks(&mut segment);
            if !segment.text.is_empty() {
                segments.push(segment);
            }
        }
    };
    for (col, cell) in cells.iter().enumerate() {
        if cell.wide_spacer {
            flush(&mut segments, &mut current);
            continue;
        }
        let ch = if cell.hidden || cell.ch.is_control() {
            ' '
        } else {
            cell.ch
        };
        let pinned = !ch.is_ascii() || cell.wide || (!cell.marks.is_empty() && !cell.hidden);
        let run = cell_run(cell, theme, base);
        if pinned {
            flush(&mut segments, &mut current);
            let mut text = String::from(ch);
            text.extend(cell.marks.iter());
            let mut run = run;
            run.len = text.len();
            segments.push(Segment {
                col,
                text,
                underlined: run.underline.is_some(),
                runs: vec![run],
            });
            continue;
        }
        match current.as_mut() {
            Some((segment, end)) if *end == col => {
                segment.text.push(ch);
                segment.underlined |= run.underline.is_some();
                match segment.runs.last_mut() {
                    Some(last) if same_style(last, &run) => last.len += 1,
                    _ => segment.runs.push(run),
                }
                *end += 1;
            }
            _ => {
                flush(&mut segments, &mut current);
                current = Some((
                    Segment {
                        col,
                        text: ch.to_string(),
                        underlined: run.underline.is_some(),
                        runs: vec![run],
                    },
                    col + 1,
                ));
            }
        }
    }
    flush(&mut segments, &mut current);
    segments
}

/// Drops the blanks at the end of an ASCII segment (a blank paints nothing) unless the segment
/// carries an underline, which blanks do draw.
fn trim_trailing_blanks(segment: &mut Segment) {
    if segment.underlined {
        return;
    }
    let kept = segment.text.trim_end_matches(' ').len();
    segment.text.truncate(kept);
    let mut remaining = kept;
    segment.runs.retain_mut(|run| {
        let take = run.len.min(remaining);
        remaining -= take;
        run.len = take;
        take > 0
    });
}

fn same_style(a: &TextRun, b: &TextRun) -> bool {
    a.color == b.color
        && a.font.weight == b.font.weight
        && a.font.style == b.font.style
        && a.underline == b.underline
}

/// The text run of one cell: foreground from the effective colors (dim multiplies the alpha by
/// 0.6), bold selects the bold weight, italic the italic style, underline a 1 px solid line in
/// the foreground color.
fn cell_run(cell: &CellSnapshot, theme: &TerminalTheme, base: &Font) -> TextRun {
    let (fg, _) = cell.display_colors();
    let mut color: Rgba = theme.resolve(fg);
    if cell.dim {
        color.a *= DIM_ALPHA;
    }
    let color: Hsla = color.into();
    let mut font = base.clone();
    if cell.bold {
        font.weight = FontWeight::BOLD;
    }
    if cell.italic {
        font.style = FontStyle::Italic;
    }
    TextRun {
        len: 1,
        font,
        color,
        background_color: None,
        underline: cell.underline.then_some(UnderlineStyle {
            color: Some(color),
            thickness: px(1.),
            wavy: false,
        }),
        strikethrough: None,
    }
}

/// A run of cells `[start, start + len)` on one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRun {
    /// The viewport row.
    pub row: usize,
    /// The first column.
    pub start: usize,
    /// The number of cells.
    pub len: usize,
}

/// Merges consecutive cells of a row that satisfy `pick` into runs, keyed by the picked value:
/// consecutive cells with an equal key share one run.
pub fn merge_runs<K: PartialEq + Copy>(
    row: usize,
    cells: &[CellSnapshot],
    pick: impl Fn(&CellSnapshot) -> Option<K>,
) -> Vec<(CellRun, K)> {
    let mut runs: Vec<(CellRun, K)> = Vec::new();
    for (col, cell) in cells.iter().enumerate() {
        let Some(key) = pick(cell) else {
            continue;
        };
        match runs.last_mut() {
            Some((run, last)) if run.start + run.len == col && *last == key => run.len += 1,
            _ => runs.push((
                CellRun {
                    row,
                    start: col,
                    len: 1,
                },
                key,
            )),
        }
    }
    runs
}

/// What the element paints for one frame, computed during prepaint.
struct Plan {
    theme: TerminalTheme,
    backgrounds: Vec<(Bounds<Pixels>, Rgba)>,
    selection: Vec<Bounds<Pixels>>,
    glyphs: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<Bounds<Pixels>>,
    scrollbar: Option<Bounds<Pixels>>,
}

/// The prepaint state: geometry, hitbox and the paint plan.
pub struct TerminalPrepaint {
    hitbox: Hitbox,
    plan: Plan,
}

/// The terminal element: draws the active tab of a [`TerminalPanel`].
pub struct TerminalElement {
    panel: Entity<TerminalPanel>,
}

impl TerminalElement {
    pub(super) fn new(panel: Entity<TerminalPanel>) -> Self {
        Self { panel }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Measures the cell width: the horizontal advance of the resolved terminal font at the
/// terminal font size, falling back to 0.6 x the font size when it cannot be measured (R5).
pub fn measure_cell_width(window: &Window, font: &Font, font_size: f32) -> f32 {
    let system = window.text_system();
    let font_id = system.resolve_font(font);
    match system.advance(font_id, px(font_size), 'm') {
        Ok(advance) if f32::from(advance.width) > 0.0 => f32::from(advance.width),
        _ => font_size * FALLBACK_CELL_WIDTH_RATIO,
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name(SharedString::from("terminal-surface")))
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        Some(accesskit::Role::Terminal)
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        node.set_label("Terminal");
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            size: size(relative(1.).into(), relative(1.).into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> TerminalPrepaint {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let theme = self.panel.read(cx).theme(cx);
        let font = terminal_font(&theme);
        let cell_width = measure_cell_width(window, &font, theme.font_size);
        let line_height = theme.line_height;
        let padding = px(PADDING);
        let origin = point(bounds.left() + padding, bounds.top() + padding);
        let inner_width = f32::from(bounds.size.width - padding * 2.0);
        let inner_height = f32::from(bounds.size.height - padding * 2.0);
        let (cols, rows) = grid_dimensions(inner_width, inner_height, cell_width, line_height);
        let geometry = GridGeometry {
            bounds,
            origin,
            cell_width,
            line_height,
            cols,
            rows,
        };
        // The emulator is resized here, during prepaint of the frame that paints the new grid;
        // no extra notify is needed.
        self.panel
            .update(cx, |panel, cx| panel.measured(geometry, cx));

        let mut plan = Plan {
            theme: theme.clone(),
            backgrounds: Vec::new(),
            selection: Vec::new(),
            glyphs: Vec::new(),
            cursor: None,
            scrollbar: None,
        };
        let panel = self.panel.read(cx);
        let Some(emulator) = panel.active_emulator() else {
            return TerminalPrepaint { hitbox, plan };
        };
        let cell_at = |row: usize, col: usize, len: usize| {
            Bounds::new(
                point(
                    origin.x + px(cell_width * col as f32),
                    origin.y + px(line_height * row as f32),
                ),
                size(px(cell_width * len as f32), px(line_height)),
            )
        };
        let font_size = px(theme.font_size);
        let text_system = window.text_system().clone();
        for row in 0..emulator.rows().min(rows) {
            let cells = emulator.line(row);
            for (run, color) in merge_runs(row, &cells, |cell| {
                let (_, bg) = cell.display_colors();
                (bg != CellColor::DefaultBackground).then(|| theme.resolve(bg))
            }) {
                let mut color = color;
                color.a = 1.0;
                plan.backgrounds
                    .push((cell_at(run.row, run.start, run.len), color));
            }
            for (run, ()) in merge_runs(row, &cells, |cell| cell.selected.then_some(())) {
                plan.selection.push(cell_at(run.row, run.start, run.len));
            }
            for segment in row_segments(&cells, &theme, &font) {
                let line = text_system.shape_line(
                    SharedString::from(segment.text),
                    font_size,
                    &segment.runs,
                    None,
                );
                plan.glyphs.push((
                    point(
                        origin.x + px(cell_width * segment.col as f32),
                        origin.y + px(line_height * row as f32),
                    ),
                    line,
                ));
            }
        }
        if let Some(CursorSnapshot { row, col }) = emulator.cursor()
            && row < rows
            && col < cols
        {
            plan.cursor = Some(cell_at(row, col, 1));
        }
        if emulator.history_lines() > 0 {
            let metrics = scrollbar_metrics(
                emulator.rows(),
                emulator.history_lines(),
                emulator.display_offset(),
                line_height,
            );
            let track = metrics.viewport;
            let thumb = (track * metrics.viewport / metrics.content)
                .max(16.0)
                .min(track);
            let top = metrics.offset / metrics.content * track;
            plan.scrollbar = Some(Bounds::new(
                point(
                    bounds.right() - px(6.0),
                    origin.y + px(top.min(track - thumb)),
                ),
                size(px(4.0), px(thumb)),
            ));
        }
        TerminalPrepaint { hitbox, plan }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut TerminalPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focused = self.panel.read(cx).focus_handle(cx).is_focused(window);
        let plan = &prepaint.plan;
        let hitbox = prepaint.hitbox.clone();
        window.set_cursor_style(CursorStyle::IBeam, &hitbox);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // Surface fill: the terminal background role.
            let mut surface = plan.theme.background;
            surface.a *= plan.theme.surface_opacity;
            window.paint_quad(fill(bounds, Hsla::from(surface)));
            // (1) Background runs, one quad per merged run.
            for (run, color) in &plan.backgrounds {
                window.paint_quad(fill(*run, Hsla::from(*color)));
            }
            // (2) The selection wash tints a cell's own background and leaves the text alone.
            for run in &plan.selection {
                window.paint_quad(fill(*run, Hsla::from(plan.theme.selection)));
            }
            // (3) Glyphs, each segment pinned at its own column.
            let line_height = px(plan.theme.line_height);
            for (origin, line) in &plan.glyphs {
                let _ = line.paint(*origin, line_height, TextAlign::Left, None, window, cx);
            }
            // (4) The cursor.
            if let Some(cell) = plan.cursor {
                if focused {
                    let mut color = plan.theme.cursor;
                    color.a *= CURSOR_BLOCK_ALPHA;
                    window.paint_quad(fill(cell, Hsla::from(color)));
                } else {
                    window.paint_quad(outline(
                        cell,
                        Hsla::from(plan.theme.cursor),
                        BorderStyle::Solid,
                    ));
                }
            }
            if let Some(thumb) = plan.scrollbar {
                let mut color = plan.theme.foreground;
                color.a *= 0.3;
                window.paint_quad(fill(thumb, Hsla::from(color)));
            }
        });

        let panel = self.panel.clone();
        window.on_mouse_event({
            let panel = panel.clone();
            let hitbox = hitbox.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble
                    && event.button == vespertine::MouseButton::Left
                    && hitbox.is_hovered(window)
                {
                    window.capture_pointer(hitbox.id);
                    panel.update(cx, |panel, cx| panel.mouse_down(event, window, cx));
                }
            }
        });
        window.on_mouse_event({
            let panel = panel.clone();
            move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble && panel.read(cx).has_gesture() {
                    panel.update(cx, |panel, cx| panel.mouse_move(event, cx));
                }
            }
        });
        window.on_mouse_event({
            let panel = panel.clone();
            move |event: &MouseUpEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble && panel.read(cx).has_gesture() {
                    panel.update(cx, |panel, cx| panel.mouse_up(event, cx));
                }
            }
        });
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                panel.update(cx, |panel, cx| panel.scroll_wheel(event, cx));
            }
        });
    }
}

use vespertine::Focusable as _;
