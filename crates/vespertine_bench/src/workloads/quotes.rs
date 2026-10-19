//! Quote-table workloads (R4, R5) and the PERF-DEV-003 definition.
//!
//! A 200-row table below a 28 px header, one 26 px view per row position, each reading the model
//! of the symbol shown at that position. Updates and sorts follow [`Schedule`].

use super::{BORDER, GREEN, INK, RED, SANS, SURFACE, WHITE, hex};
use crate::harness::{Driver, Host, Stage};
use crate::workload::{Family, Workload};
use anyhow::Result;
use std::{cell::RefCell, rc::Rc};
use vespertine::{
    AnyView, AppContext as _, Context, Div, Entity, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, UniformListScrollHandle, Window, div, point, px,
    uniform_list,
};

/// Rows of the quote tables.
pub const ROWS: usize = 200;
/// Header height, logical px.
pub const HEADER_HEIGHT: f32 = 28.0;
/// Row height, logical px.
pub const ROW_HEIGHT: f32 = 26.0;
/// Column left edges, logical px.
pub const COLUMNS: [f32; 4] = [12.0, 160.0, 300.0, 440.0];
/// Rows of the virtual table.
pub const VIRTUAL_ROWS: usize = 10_000;
/// Scroll increment of the virtual table, logical px.
pub const VIRTUAL_SCROLL_STEP: f32 = 37.0;
/// Frames between sorts.
pub const SORT_PERIOD: u64 = 15;

/// A column the table can be sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortColumn {
    /// Symbol ascending.
    Symbol,
    /// Price descending.
    Price,
    /// Volume descending.
    Volume,
}

/// Which rows a frame updates and whether it sorts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schedule {
    /// About 2% of the rows (4 of 200) update every frame.
    Sparse,
    /// 50% of the rows (100 of 200) update every frame.
    Dense,
    /// The table is re-sorted every 15 frames and exactly one price updates between sorts.
    Sorted,
    /// PERF-DEV-003: sparse for frames 0..400, dense for 400..800, sorted from 800.
    Phased,
}

/// The work of one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FramePlan {
    /// Rows whose price (and change and volume) update.
    pub updates: Vec<usize>,
    /// The sort to apply, if this frame sorts.
    pub sort: Option<SortColumn>,
}

impl Schedule {
    /// The plan of frame `f` for a table of `rows` rows.
    pub fn plan(self, f: u64, rows: usize) -> FramePlan {
        let rows = rows as u64;
        match self {
            Self::Sparse => FramePlan {
                updates: (0..4).map(|j| ((3 * f + 50 * j) % rows) as usize).collect(),
                sort: None,
            },
            Self::Dense => FramePlan {
                updates: (0..rows)
                    .filter(|r| (r + f).is_multiple_of(2))
                    .map(|r| r as usize)
                    .collect(),
                sort: None,
            },
            Self::Sorted => FramePlan {
                updates: (f % SORT_PERIOD == 7)
                    .then(|| ((f / SORT_PERIOD) % rows) as usize)
                    .into_iter()
                    .collect(),
                sort: f
                    .is_multiple_of(SORT_PERIOD)
                    .then(|| match (f / SORT_PERIOD) % 3 {
                        0 => SortColumn::Symbol,
                        1 => SortColumn::Price,
                        _ => SortColumn::Volume,
                    }),
            },
            Self::Phased if f < 400 => Self::Sparse.plan(f, rows as usize),
            Self::Phased if f < 800 => Self::Dense.plan(f, rows as usize),
            Self::Phased => Self::Sorted.plan(f, rows as usize),
        }
    }
}

/// `SYM` and the row as three digits.
pub fn symbol(row: usize) -> String {
    format!("SYM{row:03}")
}

/// The price of `row` after the update of version `version`.
pub fn price_at(row: usize, version: u64) -> f64 {
    10.0 + ((37 * row as u64 + 101 * version) % 9000) as f64 / 100.0
}

/// An integer with thousands separators.
pub fn format_volume(volume: u64) -> String {
    let digits = volume.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// One quote.
#[derive(Clone, Debug, PartialEq)]
pub struct QuoteRow {
    /// Symbol number.
    pub row: usize,
    /// Current price.
    pub price: f64,
    /// Price before the latest update.
    pub previous: f64,
    /// Current volume.
    pub volume: u64,
    /// Frame index of the latest update (0 initially).
    pub version: u64,
}

impl QuoteRow {
    /// Change of the latest update, in percent.
    pub fn change_percent(&self) -> f64 {
        (self.price - self.previous) / self.previous * 100.0
    }
}

/// The plain data behind a table: quotes and the displayed order.
pub struct QuoteBook {
    rows: Vec<QuoteRow>,
    /// Symbol shown at each position.
    pub order: Vec<usize>,
    /// Price updates applied so far.
    pub updates_applied: u64,
}

impl QuoteBook {
    /// `count` quotes at version 0 in symbol order.
    pub fn new(count: usize) -> Self {
        let rows = (0..count)
            .map(|row| {
                let price = price_at(row, 0);
                QuoteRow {
                    row,
                    price,
                    previous: price,
                    volume: 1000 * (row as u64 + 1),
                    version: 0,
                }
            })
            .collect();
        Self {
            rows,
            order: (0..count).collect(),
            updates_applied: 0,
        }
    }

    /// The quote of symbol `row`.
    pub fn row(&self, row: usize) -> &QuoteRow {
        &self.rows[row]
    }

    /// Applies the update of version `version` to symbol `row`.
    pub fn update(&mut self, row: usize, version: u64) {
        let quote = &mut self.rows[row];
        quote.previous = quote.price;
        quote.price = price_at(row, version);
        quote.volume = 1000 * (row as u64 + 1) + 17 * version;
        quote.version = version;
        self.updates_applied += 1;
    }

    /// Orders the positions by `column`.
    pub fn sort(&mut self, column: SortColumn) {
        let rows = &self.rows;
        self.order.sort_by(|&a, &b| match column {
            SortColumn::Symbol => a.cmp(&b),
            SortColumn::Price => rows[b].price.total_cmp(&rows[a].price).then(a.cmp(&b)),
            SortColumn::Volume => rows[b].volume.cmp(&rows[a].volume).then(a.cmp(&b)),
        });
    }
}

/// What a quote workload did, for tests and reports.
#[derive(Default)]
pub struct QuoteProbe {
    /// Price updates applied in each step, by frame.
    pub updates: RefCell<Vec<(u64, usize)>>,
    /// Frames that sorted.
    pub sorts: RefCell<Vec<u64>>,
    /// Row indices the virtual table rendered in its latest draw.
    pub rendered: RefCell<Vec<usize>>,
    /// Row views whose render ran, since the window opened (200-row tables only).
    pub rows_rendered: std::cell::Cell<u64>,
}

struct QuoteModel {
    data: QuoteRow,
}

struct RowView {
    position: usize,
    model: Entity<QuoteModel>,
    probe: Rc<QuoteProbe>,
}

fn cell(width: f32) -> Div {
    div().w(px(width)).flex_none()
}

fn quote_cells(quote: &QuoteRow) -> [(SharedString, Option<u32>); 4] {
    let change = quote.change_percent();
    [
        (symbol(quote.row).into(), None),
        (format!("{:.2}", quote.price).into(), None),
        (
            format!(
                "{}{:.2}%",
                if change >= 0.0 { "+" } else { "-" },
                change.abs()
            )
            .into(),
            Some(if change >= 0.0 { GREEN } else { RED }),
        ),
        (format_volume(quote.volume).into(), None),
    ]
}

fn row_element(height: f32, zebra: bool, quote: &QuoteRow) -> Div {
    let mut row = div()
        .h(px(height))
        .w_full()
        .flex()
        .flex_none()
        .items_center()
        .pl(px(COLUMNS[0]));
    if zebra {
        row = row.bg(hex(SURFACE));
    }
    for (ix, (text, color)) in quote_cells(quote).into_iter().enumerate() {
        let width = COLUMNS.get(ix + 1).map_or(160.0, |next| next - COLUMNS[ix]);
        let mut cell = cell(width);
        if let Some(color) = color {
            cell = cell.text_color(hex(color));
        }
        row = row.child(cell.child(text));
    }
    row
}

fn header_element() -> Div {
    let mut header = div()
        .h(px(HEADER_HEIGHT))
        .w_full()
        .flex()
        .flex_none()
        .items_center()
        .pl(px(COLUMNS[0]))
        .font_weight(FontWeight::BOLD)
        .border_b_1()
        .border_color(hex(BORDER));
    for (ix, label) in ["Symbol", "Price", "Change", "Volume"]
        .into_iter()
        .enumerate()
    {
        let width = COLUMNS.get(ix + 1).map_or(160.0, |next| next - COLUMNS[ix]);
        header = header.child(cell(width).child(label));
    }
    header
}

impl Render for RowView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.probe
            .rows_rendered
            .set(self.probe.rows_rendered.get() + 1);
        let quote = self.model.read(cx).data.clone();
        row_element(ROW_HEIGHT, self.position % 2 == 1, &quote)
    }
}

struct TableView {
    rows: Vec<Entity<RowView>>,
}

impl Render for TableView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(header_element())
            .children(self.rows.iter().cloned())
    }
}

struct QuoteDriver {
    schedule: Schedule,
    book: QuoteBook,
    models: Vec<Entity<QuoteModel>>,
    views: Vec<Entity<RowView>>,
    probe: Rc<QuoteProbe>,
}

impl Driver for QuoteDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let plan = self.schedule.plan(frame, ROWS);
        let Self {
            book,
            models,
            views,
            probe,
            ..
        } = self;
        stage.mutate(|app| {
            for &row in &plan.updates {
                book.update(row, frame);
                let data = book.row(row).clone();
                models[row].update(app, |model, cx| {
                    model.data = data;
                    cx.notify();
                });
            }
            if let Some(column) = plan.sort {
                book.sort(column);
                for (position, view) in views.iter().enumerate() {
                    let model = models[book.order[position]].clone();
                    view.update(app, |view, cx| {
                        if view.model != model {
                            view.model = model;
                            cx.notify();
                        }
                    });
                }
            }
        });
        probe.updates.borrow_mut().push((frame, plan.updates.len()));
        if plan.sort.is_some() {
            probe.sorts.borrow_mut().push(frame);
        }
    }
}

/// Builds a quote table window driven by `schedule`; returns the driver and a probe of what it
/// did.
pub fn quote_driver(
    harness: &mut dyn Host,
    schedule: Schedule,
) -> Result<(Box<dyn Driver>, Rc<QuoteProbe>)> {
    let book = QuoteBook::new(ROWS);
    let probe = Rc::new(QuoteProbe::default());
    let mut models = Vec::new();
    let mut views = Vec::new();
    harness.open_window(|_, cx| {
        models = (0..ROWS)
            .map(|row| {
                cx.new(|_| QuoteModel {
                    data: book.row(row).clone(),
                })
            })
            .collect();
        views = (0..ROWS)
            .map(|position| {
                let model = models[position].clone();
                let probe = probe.clone();
                cx.new(|_| RowView {
                    position,
                    model,
                    probe,
                })
            })
            .collect();
        let rows = views.clone();
        AnyView::from(cx.new(|_| TableView { rows }))
    })?;
    Ok((
        Box::new(QuoteDriver {
            schedule,
            book,
            models,
            views,
            probe: probe.clone(),
        }),
        probe,
    ))
}

struct Nothing;

impl Driver for Nothing {
    fn step(&mut self, _: u64, _: &mut Stage<'_>) {}
}

/// The unchanged window: the quote table with no step, so every frame is a forced draw.
pub fn idle() -> Workload {
    fn setup(harness: &mut dyn Host) -> Result<Box<dyn Driver>> {
        quote_driver(harness, Schedule::Sparse)?;
        Ok(Box::new(Nothing))
    }
    Workload {
        name: "idle-unchanged",
        family: Family::Baseline,
        description: "an unchanged 200-row quote table; every frame is a forced draw",
        warmup: 30,
        frames: 200,
        setup,
    }
}

struct VirtualTable {
    scroll: UniformListScrollHandle,
    probe: Rc<QuoteProbe>,
}

impl Render for VirtualTable {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let probe = self.probe.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(header_element())
            .child(
                uniform_list("virtual-quotes", VIRTUAL_ROWS, move |range, _, _| {
                    *probe.rendered.borrow_mut() = range.clone().collect();
                    range
                        .map(|row| {
                            let price = price_at(row, 0);
                            let quote = QuoteRow {
                                row,
                                price,
                                previous: price,
                                volume: 1000 * (row as u64 + 1),
                                version: 0,
                            };
                            row_element(ROW_HEIGHT, row % 2 == 1, &quote)
                        })
                        .collect::<Vec<_>>()
                })
                .track_scroll(&self.scroll)
                .flex_1(),
            )
    }
}

struct VirtualDriver {
    view: Entity<VirtualTable>,
    scroll: UniformListScrollHandle,
}

impl Driver for VirtualDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        // The offset after this step: the table has been scrolled `frame + 1` times.
        let offset = VIRTUAL_SCROLL_STEP * (frame + 1) as f32;
        let (view, scroll) = (&self.view, &self.scroll);
        stage.mutate(|app| {
            scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.0), px(-offset)));
            view.update(app, |_, cx| cx.notify());
        });
    }
}

/// The 10,000-row virtual table scrolled in 37 px increments; the probe records which rows
/// each draw rendered.
pub fn virtual_driver(harness: &mut dyn Host) -> Result<(Box<dyn Driver>, Rc<QuoteProbe>)> {
    let probe = Rc::new(QuoteProbe::default());
    let scroll = UniformListScrollHandle::new();
    let mut view = None;
    harness.open_window(|_, cx| {
        let entity = cx.new(|_| VirtualTable {
            scroll: scroll.clone(),
            probe: probe.clone(),
        });
        view = Some(entity.clone());
        AnyView::from(entity)
    })?;
    Ok((
        Box::new(VirtualDriver {
            view: view.expect("the window built the table"),
            scroll,
        }),
        probe,
    ))
}

macro_rules! quote_workload {
    ($name:literal, $family:expr, $description:literal, $schedule:expr, $warmup:expr) => {
        Workload {
            name: $name,
            family: $family,
            description: $description,
            warmup: $warmup,
            frames: 200,
            setup: |harness| Ok(quote_driver(harness, $schedule)?.0),
        }
    };
}

/// The quote workloads (R4/R5) and the PERF-DEV-003 spans.
pub fn workloads() -> Vec<Workload> {
    vec![
        quote_workload!(
            "quotes-sparse",
            Family::Quotes,
            "4 of 200 rows (2%) update every frame",
            Schedule::Sparse,
            30
        ),
        quote_workload!(
            "quotes-dense",
            Family::Quotes,
            "100 of 200 rows (50%) update every frame",
            Schedule::Dense,
            30
        ),
        quote_workload!(
            "quotes-sorted",
            Family::Quotes,
            "sort column changes every 15 frames with exactly one price update between sorts",
            Schedule::Sorted,
            30
        ),
        Workload {
            name: "quotes-virtual",
            family: Family::VirtualTable,
            description: "10,000-row virtual table, row height 26 px, scrolled in 37 px increments",
            warmup: 30,
            frames: 200,
            setup: |harness| Ok(virtual_driver(harness)?.0),
        },
        quote_workload!(
            "perf-dev-003-sparse",
            Family::Quotes,
            "PERF-DEV-003 sparse phase (frames 0..400)",
            Schedule::Phased,
            30
        ),
        quote_workload!(
            "perf-dev-003-dense",
            Family::Quotes,
            "PERF-DEV-003 dense phase (frames 400..800)",
            Schedule::Phased,
            430
        ),
        quote_workload!(
            "perf-dev-003-sorted",
            Family::Quotes,
            "PERF-DEV-003 sorted phase (frames 800..1100)",
            Schedule::Phased,
            830
        ),
    ]
}
