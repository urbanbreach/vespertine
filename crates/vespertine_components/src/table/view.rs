//! The [`Table`] component: header with resize handles and reorderable cells, virtualized rows,
//! group headings, a paging footer and an optional graph gutter (CMP-019).

use super::columns::{
    Align, CELL_PADDING_X, CELL_PADDING_Y, Column, HANDLE_WIDTH, INDICATOR_INSET, INDICATOR_WIDTH,
    IndicatorEdge, WidthMode, header_visible, primary_index, resolve_align,
};
use super::gutter::{GutterGeometry, GutterPaint, fit_gutter, graph_gutter};
use super::rows::TableRowKind;
use super::state::{SortDirection, TableState};
use std::fmt::Display;
use std::rc::Rc;
use std::time::Instant;
use vespertine::{
    AnyElement, App, AppContext as _, Context, CursorStyle, DispatchPhase, Entity, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, PathBuilder, Render, RenderOnce, Role, SharedString, StatefulInteractiveElement,
    Styled, Window, canvas, deferred, div, list, point, prelude::FluentBuilder, px,
};
use vespertine_theme::appearance::try_theme_of;
use vespertine_theme::builtin::neutral_variant;
use vespertine_theme::theme::{Appearance, ThemeVariant};

/// The theme roles of the table (THM-003): text, hairline, hover, strong border and accent.
#[derive(Clone, Debug, PartialEq)]
pub struct TableTheme {
    /// Primary text.
    pub text: Hsla,
    /// Muted text (group headings, footer).
    pub text_muted: Hsla,
    /// The row separator and header underline.
    pub hairline: Hsla,
    /// The row hover wash.
    pub hover: Hsla,
    /// The strong border (resize handle wash at 70% opacity).
    pub strong_border: Hsla,
    /// The accent (insertion indicator, selection).
    pub accent: Hsla,
    /// The overlay surface (drag ghost).
    pub overlay: Hsla,
}

impl TableTheme {
    /// The roles of a resolved variant.
    pub fn from_variant(variant: &ThemeVariant) -> Self {
        let colors = &variant.colors;
        Self {
            text: colors.text.to_hsla(),
            text_muted: colors.text_muted.to_hsla(),
            hairline: colors.border.to_hsla(),
            hover: colors.hover.to_hsla(),
            strong_border: colors.border_strong.to_hsla(),
            accent: variant.accent.primary.to_hsla(),
            overlay: colors.overlay.to_hsla(),
        }
    }

    /// The active theme of the application, or the neutral dark theme before it is initialized.
    pub fn current(cx: &App) -> Self {
        match try_theme_of(cx) {
            Some(theme) => Self::from_variant(&theme.variant),
            None => Self::default(),
        }
    }
}

impl Default for TableTheme {
    fn default() -> Self {
        Self::from_variant(&neutral_variant(Appearance::Dark))
    }
}

/// The content of a cell.
pub enum CellContent {
    /// Plain text.
    Text(SharedString),
    /// A plain number (end-aligned unless the column says otherwise).
    Number(SharedString),
    /// Arbitrary caller content: icons, graphics.
    Element(AnyElement),
}

/// A cell supplied by the caller.
pub struct Cell {
    content: CellContent,
}

impl Cell {
    /// A text cell.
    pub fn text(text: impl Into<SharedString>) -> Self {
        Self {
            content: CellContent::Text(text.into()),
        }
    }

    /// A numeric cell.
    pub fn number(value: impl Display) -> Self {
        Self {
            content: CellContent::Number(value.to_string().into()),
        }
    }

    /// A cell holding arbitrary content.
    pub fn element(element: impl IntoElement) -> Self {
        Self {
            content: CellContent::Element(element.into_any_element()),
        }
    }

    /// Whether the cell's content is a plain number.
    pub fn is_numeric(&self) -> bool {
        matches!(self.content, CellContent::Number(_))
    }
}

type CellFn = Rc<dyn Fn(&str, &str, &mut Window, &mut App) -> Option<Cell>>;
type GroupFn = Rc<dyn Fn(&str, bool, &mut Window, &mut App) -> AnyElement>;
type FooterFn = Rc<dyn Fn(bool, &mut Window, &mut App) -> AnyElement>;
type RowClickFn = Rc<dyn Fn(&str, &mut Window, &mut App)>;
type GutterFn = Rc<dyn Fn(&GutterPaint, &mut Window, &mut App)>;

/// A leading graph canvas column aligned with the rows (R8, R9).
#[derive(Clone)]
pub struct GraphColumn {
    /// The number of lanes of the graph.
    pub lanes: usize,
    paint: GutterFn,
}

impl GraphColumn {
    /// A graph column of `lanes` lanes whose marks `paint` draws for the visible rows.
    pub fn new(
        lanes: usize,
        paint: impl Fn(&GutterPaint, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            lanes,
            paint: Rc::new(paint),
        }
    }
}

/// The drag payload of a column header (INP-008): the column id and its label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnDrag {
    /// The dragged column id.
    pub id: String,
    /// Its label, shown by the ghost.
    pub label: String,
}

struct ColumnGhost {
    label: SharedString,
    theme: TableTheme,
}

impl Render for ColumnGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.0))
            .h(px(24.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .border_1()
            .border_color(self.theme.strong_border)
            .bg(self.theme.overlay)
            .text_size(px(12.0))
            .text_color(self.theme.text)
            .font_weight(vespertine::FontWeight::BOLD)
            .child(self.label.clone())
    }
}

/// How a column is laid out in rows and header.
#[derive(Clone)]
struct ColumnStyle {
    id: String,
    label: String,
    align: Option<Align>,
    width: Option<f32>,
    min: f32,
    max: f32,
    resizable: bool,
    reorderable: bool,
}

fn apply_width<E: Styled>(element: E, style: &ColumnStyle) -> E {
    match style.width {
        Some(width) => element.w(px(width)).flex_none(),
        None => {
            let mut element = element.flex_1().flex_basis(px(0.0)).min_w(px(style.min));
            if style.max.is_finite() {
                element = element.max_w(px(style.max));
            }
            element
        }
    }
}

fn justify<E: Styled>(element: E, align: Align) -> E {
    match align {
        Align::Start => element.justify_start(),
        Align::Center => element.justify_center(),
        Align::End => element.justify_end(),
    }
}

/// A data table over a [`TableState`].
#[derive(IntoElement)]
pub struct Table {
    state: Entity<TableState>,
    cell: CellFn,
    group_header: Option<GroupFn>,
    footer: Option<FooterFn>,
    theme: Option<TableTheme>,
    selected: Option<String>,
    hover: bool,
    on_row_click: Option<RowClickFn>,
    header_trailing: Option<AnyElement>,
    graph: Option<GraphColumn>,
    padding: (f32, f32),
    header_height: f32,
}

impl Table {
    /// A table over `state`; `cell(row id, column id)` supplies the cells of data rows (missing
    /// cells render empty). The row id is the item id (without the `item:` prefix).
    pub fn new(
        state: &Entity<TableState>,
        cell: impl Fn(&str, &str, &mut Window, &mut App) -> Option<Cell> + 'static,
    ) -> Self {
        Self {
            state: state.clone(),
            cell: Rc::new(cell),
            group_header: None,
            footer: None,
            theme: None,
            selected: None,
            hover: true,
            on_row_click: None,
            header_trailing: None,
            graph: None,
            padding: (CELL_PADDING_X, CELL_PADDING_Y),
            header_height: 32.0,
        }
    }

    /// The content of group headings `(key, collapsed)`; the default shows the key in bold.
    pub fn group_header(
        mut self,
        header: impl Fn(&str, bool, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.group_header = Some(Rc::new(header));
        self
    }

    /// The content of the paging footer `(loading)`; the default is a "Load more" control.
    pub fn footer(
        mut self,
        footer: impl Fn(bool, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.footer = Some(Rc::new(footer));
        self
    }

    /// Replaces the theme roles (by default the active theme of the application).
    pub fn theme(mut self, theme: TableTheme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// The caller-owned selected row id (the item id).
    pub fn selected(mut self, id: Option<String>) -> Self {
        self.selected = id;
        self
    }

    /// Switches the row hover wash on or off (on by default).
    pub fn row_hover(mut self, hover: bool) -> Self {
        self.hover = hover;
        self
    }

    /// Called with the item id when a data row is clicked.
    pub fn on_row_click(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_row_click = Some(Rc::new(handler));
        self
    }

    /// A slot at the right end of the header, for example an active-filter count.
    pub fn header_trailing(mut self, element: impl IntoElement) -> Self {
        self.header_trailing = Some(element.into_any_element());
        self
    }

    /// A leading graph canvas column aligned with the rows.
    pub fn graph(mut self, graph: GraphColumn) -> Self {
        self.graph = Some(graph);
        self
    }

    /// Overrides the cell padding (horizontal, vertical).
    pub fn cell_padding(mut self, horizontal: f32, vertical: f32) -> Self {
        self.padding = (horizontal, vertical);
        self
    }
}

fn sort_indicator(direction: SortDirection, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let half = px(3.5);
            let (a, b, c) = match direction {
                SortDirection::Ascending => (
                    point(center.x - half, center.y + px(2.0)),
                    point(center.x + half, center.y + px(2.0)),
                    point(center.x, center.y - px(3.0)),
                ),
                SortDirection::Descending => (
                    point(center.x - half, center.y - px(2.0)),
                    point(center.x + half, center.y - px(2.0)),
                    point(center.x, center.y + px(3.0)),
                ),
            };
            let mut builder = PathBuilder::fill();
            builder.move_to(a);
            builder.line_to(b);
            builder.line_to(c);
            builder.close();
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w(px(10.0))
    .h(px(10.0))
    .flex_none()
}

fn chevron_glyph(collapsed: bool, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let arm = px(2.5);
            let (a, b, c) = if collapsed {
                (
                    point(center.x - arm * 0.6, center.y - arm * 1.2),
                    point(center.x + arm * 0.6, center.y),
                    point(center.x - arm * 0.6, center.y + arm * 1.2),
                )
            } else {
                (
                    point(center.x - arm * 1.2, center.y - arm * 0.6),
                    point(center.x, center.y + arm * 0.6),
                    point(center.x + arm * 1.2, center.y - arm * 0.6),
                )
            };
            let mut builder = PathBuilder::stroke(px(1.25));
            builder.move_to(a);
            builder.line_to(b);
            builder.line_to(c);
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w(px(12.0))
    .h(px(12.0))
    .flex_none()
}

fn cell_element(
    index: usize,
    style: &ColumnStyle,
    cell: Option<Cell>,
    padding: (f32, f32),
    theme: &TableTheme,
) -> AnyElement {
    let numeric = cell.as_ref().is_some_and(Cell::is_numeric);
    let align = resolve_align(style.align, numeric);
    let base = div()
        .id(("table-cell", index))
        .role(Role::Cell)
        .relative()
        .flex()
        .items_center()
        .px(px(padding.0))
        .py(px(padding.1))
        .overflow_hidden()
        .text_size(px(12.0))
        .text_color(theme.text);
    let base = justify(apply_width(base, style), align);
    match cell.map(|cell| cell.content) {
        Some(CellContent::Text(text) | CellContent::Number(text)) => base
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(text),
            )
            .into_any_element(),
        Some(CellContent::Element(element)) => base.child(element).into_any_element(),
        None => base.into_any_element(),
    }
}

impl RenderOnce for Table {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self
            .theme
            .clone()
            .unwrap_or_else(|| TableTheme::current(cx));
        let state_entity = self.state.clone();
        let (
            styles,
            show_header,
            row_height,
            list_state,
            sort,
            indicator,
            graph_geometry,
            resizing,
        ) = {
            let state = self.state.read(cx);
            let visible = state.visible_columns();
            let primary = primary_index(&visible);
            let styles: Vec<ColumnStyle> = visible
                .iter()
                .enumerate()
                .map(|(ix, column)| {
                    let is_primary = primary == Some(ix);
                    let (min, max) = (
                        column.effective_min(is_primary),
                        column.effective_max(is_primary),
                    );
                    let width = state
                        .widths()
                        .get(&column.id)
                        .copied()
                        .or(match column.width {
                            WidthMode::Fixed(width) => Some(width),
                            WidthMode::Flex => None,
                        })
                        .map(|width| width.max(min).min(max));
                    ColumnStyle {
                        id: column.id.clone(),
                        label: column.label.clone(),
                        align: column.align,
                        width,
                        min,
                        max,
                        resizable: column.resizable,
                        reorderable: column.reorderable,
                    }
                })
                .collect();
            let owned: Vec<Column> = visible.iter().map(|column| (*column).clone()).collect();
            let graph_geometry = self.graph.as_ref().map(|graph| {
                if state.header_width() <= 0.0 {
                    // Not measured yet: show the natural geometry without fitting or morphing.
                    return GutterGeometry::natural(graph.lanes);
                }
                let previous = state.gutter_geometry();
                let container =
                    state.header_width() + previous.map_or(0.0, |geometry| geometry.width);
                let widths = state.rendered_widths();
                let optional: f32 = widths
                    .iter()
                    .enumerate()
                    .filter(|(ix, _)| primary != Some(*ix))
                    .map(|(_, width)| *width)
                    .sum();
                let target = fit_gutter(graph.lanes, container, optional, previous.as_ref());
                state.record_gutter(target);
                let (shown, animating) = state.step_gutter(target, Instant::now());
                if animating {
                    window.request_animation_frame();
                }
                shown
            });
            (
                styles,
                header_visible(&owned),
                state.row_height(),
                state.list_state().clone(),
                state.sort().cloned(),
                state.insertion_indicator().filter(|_| cx.has_active_drag()),
                graph_geometry,
                state.is_resizing(),
            )
        };
        let styles = Rc::new(styles);
        let (pad_x, pad_y) = self.padding;
        let height = row_height.map(px);

        // ---- header ----
        let header = show_header.then(|| {
            let mut cells: Vec<AnyElement> = Vec::new();
            for (ix, style) in styles.iter().enumerate() {
                let sorted = sort
                    .as_ref()
                    .filter(|sort| sort.column == style.id)
                    .map(|sort| sort.direction);
                let click_state = state_entity.clone();
                let click_id = style.id.clone();
                let ghost_theme = theme.clone();
                let drag = ColumnDrag {
                    id: style.id.clone(),
                    label: style.label.clone(),
                };
                let mut cell = div()
                    .id(("table-head", ix))
                    .role(Role::ColumnHeader)
                    .aria_label(SharedString::from(style.label.clone()))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(pad_x))
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .font_weight(vespertine::FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(style.label.clone())),
                    )
                    .when_some(sorted, |cell, direction| {
                        cell.child(sort_indicator(direction, theme.text_muted))
                    })
                    .when(indicator.is_some_and(|(target, _)| target == ix), |cell| {
                        let edge = indicator.map(|(_, edge)| edge);
                        let bar = div()
                            .absolute()
                            .top(px(INDICATOR_INSET))
                            .bottom(px(INDICATOR_INSET))
                            .w(px(INDICATOR_WIDTH))
                            .rounded(px(INDICATOR_WIDTH / 2.0))
                            .bg(theme.accent);
                        cell.child(match edge {
                            Some(IndicatorEdge::Trailing) => bar.right_0(),
                            _ => bar.left_0(),
                        })
                    })
                    .on_click(move |_, _, cx| {
                        click_state.update(cx, |state, cx| state.request_sort(&click_id, cx));
                    });
                cell = justify(apply_width(cell, style), style.align.unwrap_or_default());
                let cell = if style.reorderable {
                    cell.on_drag(drag, move |drag, _, _, cx| {
                        cx.new(|_| ColumnGhost {
                            label: drag.label.clone().into(),
                            theme: ghost_theme.clone(),
                        })
                    })
                    .into_any_element()
                } else {
                    cell.into_any_element()
                };
                cells.push(cell);
                if ix + 1 < styles.len() && style.resizable && styles[ix + 1].resizable {
                    let down_state = state_entity.clone();
                    let up_state = state_entity.clone();
                    let left_id = style.id.clone();
                    let wash = theme.strong_border.opacity(0.7);
                    cells.push(
                        div()
                            .id(("table-handle", ix))
                            .relative()
                            .w(px(0.0))
                            .h_full()
                            .flex_none()
                            .child(deferred(
                                div()
                                    .id(("table-handle-hit", ix))
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(px(-HANDLE_WIDTH / 2.0))
                                    .w(px(HANDLE_WIDTH))
                                    .cursor(CursorStyle::ResizeColumn)
                                    .hover(move |style| style.bg(wash))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        move |event: &MouseDownEvent, window, cx| {
                                            window.prevent_default();
                                            cx.stop_propagation();
                                            let x = f32::from(event.position.x);
                                            down_state.update(cx, |state, cx| {
                                                state.begin_resize(&left_id, x, cx)
                                            });
                                        },
                                    )
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        move |event: &MouseUpEvent, _, cx| {
                                            if event.click_count == 2 {
                                                up_state
                                                    .update(cx, |state, cx| state.reset_widths(cx));
                                            }
                                        },
                                    ),
                            ))
                            .into_any_element(),
                    );
                }
            }
            let measure_state = state_entity.clone();
            let drag_state = state_entity.clone();
            let drop_state = state_entity.clone();
            let resize_state = state_entity.clone();
            let spacer = graph_geometry
                .map(|geometry| div().w(px(geometry.width)).flex_none().into_any_element());
            div()
                .id("table-header")
                .role(Role::Row)
                .flex()
                .flex_row()
                .w_full()
                .h(px(self.header_height))
                .flex_none()
                .border_b_1()
                .border_color(theme.hairline)
                .children(spacer)
                .child(
                    div()
                        .id("table-header-cells")
                        .relative()
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            canvas(
                                move |bounds, _, cx| {
                                    let width = f32::from(bounds.size.width);
                                    let changed = {
                                        let state = measure_state.read(cx);
                                        let changed = (state.header_width() - width).abs() > 0.5;
                                        state.record_header_width(width);
                                        changed
                                    };
                                    if changed {
                                        let state = measure_state;
                                        cx.defer(move |cx| state.update(cx, |_, cx| cx.notify()));
                                    }
                                },
                                move |_, _, window, _| {
                                    if resizing {
                                        let moves = resize_state.clone();
                                        window.on_mouse_event(
                                            move |event: &MouseMoveEvent, phase, _, cx| {
                                                if phase == DispatchPhase::Capture {
                                                    let x = f32::from(event.position.x);
                                                    moves.update(cx, |state, cx| {
                                                        state.update_resize(x, cx)
                                                    });
                                                }
                                            },
                                        );
                                        let ups = resize_state;
                                        window.on_mouse_event(
                                            move |_: &MouseUpEvent, phase, _, cx| {
                                                if phase == DispatchPhase::Capture {
                                                    ups.update(cx, |state, cx| {
                                                        state.end_resize(cx)
                                                    });
                                                }
                                            },
                                        );
                                    }
                                },
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                        )
                        .children(cells)
                        .on_drag_move::<ColumnDrag>(move |event, _, cx| {
                            let position = event.event.position;
                            let drag = event.drag(cx).clone();
                            if event.bounds.contains(&position) {
                                let x = f32::from(position.x - event.bounds.left());
                                drag_state.update(cx, |state, cx| {
                                    state.drag_column_over(&drag.id, x, cx)
                                });
                            } else {
                                drag_state.update(cx, |state, cx| state.clear_drag_over(cx));
                            }
                        })
                        .on_drop::<ColumnDrag>(move |_, _, cx| {
                            drop_state.update(cx, |state, cx| {
                                state.commit_reorder(cx);
                            });
                        }),
                )
                .children(self.header_trailing)
                .into_any_element()
        });

        // ---- rows ----
        let rows_state = state_entity;
        let cell_fn = self.cell.clone();
        let group_fn = self.group_header.clone();
        let footer_fn = self.footer.clone();
        let row_click = self.on_row_click.clone();
        let selected = self.selected.clone();
        let hover_enabled = self.hover;
        let row_styles = styles;
        let row_theme = theme.clone();
        let render_item = move |ix: usize, window: &mut Window, cx: &mut App| -> AnyElement {
            let Some(row) = rows_state.read(cx).rows().get(ix).cloned() else {
                return div().into_any_element();
            };
            match row.kind {
                TableRowKind::Item => {
                    let item_id = row.item_id().unwrap_or_default().to_owned();
                    let cells: Vec<AnyElement> = row_styles
                        .iter()
                        .enumerate()
                        .map(|(column_ix, style)| {
                            let cell = cell_fn(&item_id, &style.id, window, cx);
                            cell_element(column_ix, style, cell, (pad_x, pad_y), &row_theme)
                        })
                        .collect();
                    let is_selected = selected.as_deref() == Some(item_id.as_str());
                    let hover = row_theme.hover;
                    let click = row_click.clone();
                    div()
                        .id(("table-row", ix))
                        .role(Role::Row)
                        .flex()
                        .flex_row()
                        .items_stretch()
                        .w_full()
                        .when_some(height, |row, height| row.h(height))
                        .border_b_1()
                        .border_color(row_theme.hairline)
                        .when(is_selected, |row| row.bg(row_theme.accent.opacity(0.16)))
                        .when(hover_enabled && !is_selected, |row| {
                            row.hover(move |style| style.bg(hover))
                        })
                        .children(cells)
                        .on_click(move |_, window, cx| {
                            if let Some(click) = &click {
                                click(&item_id, window, cx);
                            }
                        })
                        .into_any_element()
                }
                TableRowKind::Group => {
                    let key = row.group_key().unwrap_or_default().to_owned();
                    let collapsed = rows_state.read(cx).collapsed().contains(&key);
                    let content = match &group_fn {
                        Some(header) => header(&key, collapsed, window, cx),
                        None => div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(chevron_glyph(collapsed, row_theme.text_muted))
                            .child(
                                div()
                                    .font_weight(vespertine::FontWeight::BOLD)
                                    .child(SharedString::from(key.clone())),
                            )
                            .into_any_element(),
                    };
                    let toggle = rows_state.clone();
                    div()
                        .id(("table-group", ix))
                        .flex()
                        .items_center()
                        .w_full()
                        .when_some(height, |row, height| row.h(height))
                        .px(px(pad_x))
                        .border_b_1()
                        .border_color(row_theme.hairline)
                        .text_size(px(12.0))
                        .text_color(row_theme.text)
                        .cursor(CursorStyle::PointingHand)
                        .child(content)
                        .on_click(move |_, _, cx| {
                            toggle.update(cx, |state, cx| state.toggle_group(&key, false, cx));
                        })
                        .into_any_element()
                }
                TableRowKind::Footer => {
                    let loading = rows_state.read(cx).loading_more();
                    let content = match &footer_fn {
                        Some(footer) => footer(loading, window, cx),
                        None => div()
                            .child(if loading { "Loading…" } else { "Load more" })
                            .into_any_element(),
                    };
                    let more = rows_state.clone();
                    div()
                        .id(("table-footer", ix))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w_full()
                        .when_some(height, |row, height| row.h(height))
                        .text_size(px(12.0))
                        .text_color(row_theme.text_muted)
                        .when(!loading, |row| row.cursor(CursorStyle::PointingHand))
                        .child(content)
                        .on_click(move |_, _, cx| {
                            if !more.read(cx).loading_more() {
                                more.update(cx, |state, cx| state.request_load_more(cx));
                            }
                        })
                        .into_any_element()
                }
            }
        };
        let gutter = self
            .graph
            .as_ref()
            .zip(graph_geometry)
            .map(|(graph, geometry)| {
                let paint = graph.paint.clone();
                graph_gutter(
                    list_state.clone(),
                    px(row_height.unwrap_or(36.0)),
                    geometry,
                    move |gutter_paint, window, cx| paint(gutter_paint, window, cx),
                )
            });
        div()
            .id("table")
            .role(Role::Table)
            .flex()
            .flex_col()
            .size_full()
            .text_size(px(12.0))
            .text_color(theme.text)
            .children(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .children(gutter)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(list(list_state, render_item).size_full()),
                    ),
            )
    }
}
