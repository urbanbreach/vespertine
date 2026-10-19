//! CMP-019 A6: a resizable, reorderable, grouped and paged commit-history table with a graph
//! gutter.
//!
//! Drag the dividers between the header cells to resize columns (double-click a divider to
//! reset every width), drag a header cell to reorder the columns, click a header to sort,
//! click a month heading to collapse its group and press "Load more" to append the next page of
//! 100 commits. Scroll first: the first visible row keeps its place through every one of those
//! changes. The leading canvas column draws lanes and nodes aligned with the rows.
//!
//! Run with: `cargo run -p vespertine_components --example data_table`

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use vespertine::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window, WindowBounds, WindowOptions, div, fill, point, px, rgb, size,
};
use vespertine_components::table::{
    Cell, Column, GraphColumn, GroupSpec, Sort, SortDirection, Table, TableEvent, TableRow,
    TableState,
};
use vespertine_platform::application;

const PAGE_SIZE: usize = 100;
const TOTAL: usize = 450;
const LANES: usize = 4;

#[derive(Clone)]
struct Commit {
    id: usize,
    message: String,
    author: &'static str,
    month: String,
    lines: u32,
    lane: usize,
}

fn commits() -> Vec<Commit> {
    const AUTHORS: [&str; 4] = [
        "Ada Lovelace",
        "Grace Hopper",
        "Alan Turing",
        "Edsger Dijkstra",
    ];
    const VERBS: [&str; 5] = ["Fix", "Add", "Refactor", "Document", "Speed up"];
    const THINGS: [&str; 5] = [
        "scheduler",
        "layout cache",
        "text shaping",
        "list anchors",
        "tree model",
    ];
    (0..TOTAL)
        .map(|id| Commit {
            id,
            message: format!("{} {} ({})", VERBS[id % 5], THINGS[(id / 5) % 5], 1000 + id),
            author: AUTHORS[(id * 7) % 4],
            month: format!("2026-{:02}", 12 - id / 40),
            lines: ((id * 37) % 480) as u32 + 3,
            lane: (id * 5 / 3) % LANES,
        })
        .collect()
}

struct DataTableExample {
    state: Entity<TableState>,
    all: Vec<Commit>,
    by_id: Rc<RefCell<HashMap<String, Commit>>>,
    loaded: usize,
    sort: Option<Sort>,
}

impl DataTableExample {
    fn new(cx: &mut Context<Self>) -> Self {
        let columns = vec![
            Column::new("message", "Message"),
            Column::new("author", "Author").width(150.0).min_width(80.0),
            Column::new("month", "Month").width(90.0),
            Column::new("lines", "Lines").width(80.0).min_width(50.0),
        ];
        let state = cx.new(|_| TableState::new(columns, Some(36.0)));
        cx.subscribe(&state, |this, _, event: &TableEvent, cx| match event {
            TableEvent::LoadMoreRequested => {
                this.loaded = (this.loaded + PAGE_SIZE).min(TOTAL);
                this.refresh(cx);
            }
            TableEvent::SortRequested(column) => {
                let direction = match &this.sort {
                    Some(sort)
                        if sort.column == *column && sort.direction == SortDirection::Ascending =>
                    {
                        SortDirection::Descending
                    }
                    _ => SortDirection::Ascending,
                };
                this.sort = Some(Sort {
                    column: column.clone(),
                    direction,
                });
                this.refresh(cx);
            }
            TableEvent::ColumnResized(widths) => println!("resized {widths:?}"),
            TableEvent::ColumnReordered(order) => println!("reordered {order:?}"),
            TableEvent::ColumnsReset => println!("widths reset"),
            TableEvent::GroupToggled(key) => println!("toggled {key}"),
        })
        .detach();
        let mut example = Self {
            state,
            all: commits(),
            by_id: Rc::new(RefCell::new(HashMap::new())),
            loaded: PAGE_SIZE,
            sort: None,
        };
        example.refresh(cx);
        example
    }

    /// Re-supplies the rows in the requested order; the table keeps the visible anchor.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut visible: Vec<Commit> = self.all[..self.loaded].to_vec();
        if let Some(sort) = &self.sort {
            visible.sort_by(|a, b| {
                let ordering = match sort.column.as_str() {
                    "author" => a.author.cmp(b.author),
                    "lines" => a.lines.cmp(&b.lines),
                    "month" => a.month.cmp(&b.month),
                    _ => a.message.cmp(&b.message),
                };
                if sort.direction == SortDirection::Descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            });
        }
        let mut groups: Vec<GroupSpec> = Vec::new();
        for commit in &visible {
            match groups.last_mut() {
                Some(group) if group.key.as_deref() == Some(commit.month.as_str()) => {
                    group.items.push(TableRow::item(&commit.id.to_string()));
                }
                _ => groups.push(GroupSpec::new(
                    &commit.month,
                    vec![TableRow::item(&commit.id.to_string())],
                )),
            }
        }
        *self.by_id.borrow_mut() = visible
            .into_iter()
            .map(|commit| (commit.id.to_string(), commit))
            .collect();
        let sort = self.sort.clone();
        let has_more = self.loaded < TOTAL;
        self.state.update(cx, |state, cx| {
            state.set_sort(sort, cx);
            state.set_data(groups, has_more, false, cx);
        });
        cx.notify();
    }
}

impl Render for DataTableExample {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let by_id = self.by_id.clone();
        let state = self.state.clone();
        let graph_state = self.state.clone();
        let by_id_for_graph = self.by_id.clone();
        let table = Table::new(&self.state, move |row, column, _, _| {
            let by_id = by_id.borrow();
            let commit = by_id.get(row)?;
            Some(match column {
                "message" => Cell::text(commit.message.clone()),
                "author" => Cell::text(commit.author),
                "month" => Cell::text(commit.month.clone()),
                "lines" => Cell::number(commit.lines),
                _ => return None,
            })
        })
        .graph(GraphColumn::new(LANES, move |gutter, window, cx| {
            let rows = graph_state.read(cx).rows().to_vec();
            let by_id = by_id_for_graph.borrow();
            let palette = [0x5b9bd5, 0x70ad47, 0xed7d31, 0xa66bd8];
            for ix in gutter.rows.clone() {
                let Some(commit) = rows
                    .get(ix)
                    .and_then(|row| row.item_id())
                    .and_then(|id| by_id.get(id))
                else {
                    continue;
                };
                let top = gutter.row_top(ix);
                // Marks that cross the row boundary overlap each neighbor row by 0.75 px.
                let overlap = px(vespertine_components::table::gutter::ROW_OVERLAP);
                for lane in [0, commit.lane] {
                    let x = gutter.lane_x(lane);
                    let line = Bounds::new(
                        point(x - px(0.75), top - overlap),
                        size(px(1.5), gutter.row_height + overlap * 2.0),
                    );
                    window.paint_quad(fill(line, rgb(palette[lane % palette.len()])));
                }
                let x = gutter.lane_x(commit.lane);
                let radius = px(vespertine_components::table::gutter::NODE_RADIUS);
                let node = Bounds::new(
                    point(x - radius, top + gutter.row_height / 2.0 - radius),
                    size(radius * 2.0, radius * 2.0),
                );
                window.paint_quad(
                    fill(node, rgb(palette[commit.lane % palette.len()])).corner_radii(radius),
                );
            }
        }));
        let _ = state;
        div()
            .size_full()
            .bg(rgb(0x181a1f))
            .text_color(rgb(0xe6e8ec))
            .child(table)
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(860.0), px(560.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(DataTableExample::new),
        )
        .expect("window");
        cx.activate(true);
    });
}
