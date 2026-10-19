//! CMP-019 view tests on the offscreen test host: handles, reordering, alignment of the graph
//! gutter with the rows (A7), numeric alignment and the header rule (A9).

use std::cell::RefCell;
use std::rc::Rc;
use vespertine::{
    AppContext as _, Bounds, Context, Entity, IntoElement, Modifiers, MouseButton, MouseUpEvent,
    ParentElement as _, PlatformInput, Render, Styled as _, Window, div, fill, point, px, rgb,
    size,
};
use vespertine_components::table::{
    Align, Cell, Column, GraphColumn, GroupSpec, Sort, SortDirection, Table, TableEvent, TableRow,
    TableState,
};
use vespertine_test_host::{HostOptions, TestHost};

type CellFn = Rc<dyn Fn(&str, &str) -> Option<Cell>>;
type Config = Rc<dyn Fn(Table) -> Table>;

struct TableRoot {
    state: Entity<TableState>,
    cells: CellFn,
    config: Config,
}

impl Render for TableRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let cells = self.cells.clone();
        let table = Table::new(&self.state, move |row, column, _, _| cells(row, column));
        div()
            .size_full()
            .bg(rgb(0x101010))
            .child((self.config)(table))
    }
}

struct Harness {
    host: TestHost,
    state: Entity<TableState>,
    events: Rc<RefCell<Vec<TableEvent>>>,
}

impl Harness {
    fn new(
        width: f64,
        height: f64,
        columns: Vec<Column>,
        row_height: Option<f32>,
        cells: impl Fn(&str, &str) -> Option<Cell> + 'static,
        config: impl Fn(Table) -> Table + 'static,
    ) -> Harness {
        let slot: Rc<RefCell<Option<Entity<TableState>>>> = Rc::new(RefCell::new(None));
        let build_slot = slot.clone();
        let cells: CellFn = Rc::new(cells);
        let config: Config = Rc::new(config);
        let mut host = TestHost::with_view(HostOptions::with_size(width, height), move |_, cx| {
            let state = cx.new(|_| TableState::new(columns, row_height));
            *build_slot.borrow_mut() = Some(state.clone());
            cx.new(|_| TableRoot {
                state,
                cells,
                config,
            })
        })
        .expect("test host");
        let state = slot.borrow_mut().take().expect("state");
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        host.app().update(|cx| {
            cx.subscribe(&state, move |_, event: &TableEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
            .detach();
        });
        Harness {
            host,
            state,
            events,
        }
    }

    fn update<R>(&mut self, f: impl FnOnce(&mut TableState, &mut Context<TableState>) -> R) -> R {
        let state = self.state.clone();
        let result = self.host.app().update(|cx| state.update(cx, f));
        self.settle();
        result
    }

    fn read<R>(&mut self, f: impl FnOnce(&TableState) -> R) -> R {
        let state = self.state.clone();
        self.host.app().update(|cx| f(state.read(cx)))
    }

    /// Frames until the header width the table measures has propagated.
    fn settle(&mut self) {
        for _ in 0..3 {
            self.host.flush();
        }
    }

    fn mouse_up_with_clicks(&mut self, x: f32, y: f32, click_count: usize) {
        let window = self.host.window();
        let input = PlatformInput::MouseUp(MouseUpEvent {
            position: point(px(x), px(y)),
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count,
        });
        self.host
            .app()
            .simulate_platform_input(window, input)
            .unwrap();
        self.host.app().run_until_parked();
        self.settle();
    }
}

fn commit_columns() -> Vec<Column> {
    vec![
        Column::new("id", "Id").width(60.0),
        Column::new("msg", "Message"),
        Column::new("author", "Author").width(120.0),
        Column::new("lines", "Lines").width(80.0),
    ]
}

fn text_cells(row: &str, column: &str) -> Option<Cell> {
    match column {
        "id" => Some(Cell::text(row.to_owned())),
        "msg" => Some(Cell::text(format!("message {row}"))),
        "author" => Some(Cell::text("ada")),
        "lines" => Some(Cell::number(row.len())),
        _ => None,
    }
}

fn rows(count: usize) -> Vec<GroupSpec> {
    vec![GroupSpec::ungrouped(
        (0..count).map(|n| TableRow::item(&n.to_string())).collect(),
    )]
}

fn commit_harness() -> Harness {
    let mut harness = Harness::new(
        600.0,
        300.0,
        commit_columns(),
        Some(36.0),
        text_cells,
        |table| table,
    );
    harness.update(|state, cx| state.set_data(rows(30), false, false, cx));
    harness
}

fn header_y() -> f32 {
    16.0
}

/// The x of the divider right of the visible column `index`.
fn divider(harness: &mut Harness, index: usize) -> f32 {
    harness.read(|state| state.rendered_widths()[..=index].iter().sum())
}

// ---- A3: resize handles --------------------------------------------------------------------

#[test]
fn a3_dragging_a_handle_next_to_the_primary_column_resizes_only_the_right_column() {
    vespertine_test_host::require_backend!();
    let mut harness = commit_harness();
    // Handle between Message (the primary column) and Author.
    let x = divider(&mut harness, 1);
    assert!(harness.read(|state| state.header_width()) > 590.0);
    harness.host.mouse_down(f64::from(x), 16.0, 0, "").unwrap();
    assert!(harness.read(|state| state.is_resizing()));
    harness
        .host
        .mouse_move(f64::from(x - 30.0), 16.0, Some(0), "")
        .unwrap();
    assert_eq!(
        harness.read(|state| state.widths().get("author").copied()),
        Some(150.0)
    );
    assert!(
        harness.read(|state| state.widths().get("msg").is_none()),
        "the primary column stays flexible"
    );
    harness
        .host
        .mouse_up(f64::from(x - 30.0), 16.0, 0, "")
        .unwrap();
    assert!(!harness.read(|state| state.is_resizing()));
    let events = harness.events.borrow().clone();
    assert_eq!(events.len(), 1);
    let TableEvent::ColumnResized(widths) = &events[0] else {
        panic!("resized event");
    };
    assert_eq!(widths.get("author"), Some(&150.0));
}

#[test]
fn a3_a_handle_between_two_fixed_columns_keeps_their_total() {
    vespertine_test_host::require_backend!();
    let mut harness = commit_harness();
    let x = divider(&mut harness, 2);
    harness.host.mouse_down(f64::from(x), 16.0, 0, "").unwrap();
    harness
        .host
        .mouse_move(f64::from(x + 100.0), 16.0, Some(0), "")
        .unwrap();
    // Author 120 -> 200 but Lines (80, min 0) can only give 80: the delta is limited to 80.
    let widths = harness.read(|state| state.widths().clone());
    assert_eq!(widths.get("author"), Some(&200.0));
    assert_eq!(widths.get("lines"), Some(&0.0));
    harness
        .host
        .mouse_up(f64::from(x + 100.0), 16.0, 0, "")
        .unwrap();
}

#[test]
fn a3_a_single_press_and_release_only_ends_the_drag_and_a_double_click_resets() {
    vespertine_test_host::require_backend!();
    let mut harness = commit_harness();
    let x = divider(&mut harness, 1);
    harness.host.click(f64::from(x), 16.0, 0, "").unwrap();
    assert!(!harness.read(|state| state.is_resizing()));
    assert!(
        harness.events.borrow().is_empty(),
        "no event for a plain press and release: {:?}",
        harness.events.borrow()
    );
    // Resize, then double-click the handle (click count 2 on release).
    harness.host.mouse_down(f64::from(x), 16.0, 0, "").unwrap();
    harness
        .host
        .mouse_move(f64::from(x - 40.0), 16.0, Some(0), "")
        .unwrap();
    harness
        .host
        .mouse_up(f64::from(x - 40.0), 16.0, 0, "")
        .unwrap();
    assert!(!harness.read(|state| state.widths().is_empty()));
    harness.events.borrow_mut().clear();
    let x = divider(&mut harness, 1);
    harness.host.mouse_down(f64::from(x), 16.0, 0, "").unwrap();
    harness.mouse_up_with_clicks(x, 16.0, 2);
    assert_eq!(*harness.events.borrow(), [TableEvent::ColumnsReset]);
    assert!(
        harness.read(|state| state.widths().is_empty()),
        "default widths restored"
    );
    // Pressing a handle does not start the header reordering.
    assert!(harness.read(|state| state.drag_over().is_none()));
}

// ---- A2: reorder through the header --------------------------------------------------------

#[test]
fn a2_dragging_a_header_cell_over_another_shows_the_indicator_and_commits_on_release() {
    vespertine_test_host::require_backend!();
    let mut harness = commit_harness();
    let author_x = divider(&mut harness, 1) + 40.0;
    let msg_x = 60.0_f64 + 10.0;
    harness
        .host
        .mouse_down(f64::from(author_x), f64::from(header_y()), 0, "")
        .unwrap();
    harness
        .host
        .mouse_move(
            f64::from(author_x - 4.0),
            f64::from(header_y()),
            Some(0),
            "",
        )
        .unwrap();
    harness
        .host
        .mouse_move(msg_x, f64::from(header_y()), Some(0), "")
        .unwrap();
    let over = harness.read(|state| state.drag_over().cloned());
    let over = over.expect("dragging over the header");
    assert_eq!(
        (over.dragged.as_str(), over.over.as_str()),
        ("author", "msg")
    );
    use vespertine_components::table::IndicatorEdge;
    assert_eq!(
        harness.read(|state| state.insertion_indicator()),
        Some((1, IndicatorEdge::Leading)),
        "moving earlier shows the leading edge of the target"
    );
    harness
        .host
        .mouse_up(msg_x, f64::from(header_y()), 0, "")
        .unwrap();
    let order = harness.read(|state| state.column_order());
    assert_eq!(order, ["id", "author", "msg", "lines"]);
    assert_eq!(
        *harness.events.borrow(),
        [TableEvent::ColumnReordered(
            ["id", "author", "msg", "lines"]
                .iter()
                .map(|id| (*id).to_owned())
                .collect()
        )]
    );
    // Identity survives: the width and flags of the moved column are unchanged.
    let author = harness.read(|state| {
        state
            .columns()
            .iter()
            .find(|c| c.id == "author")
            .cloned()
            .unwrap()
    });
    assert_eq!(
        author.width,
        vespertine_components::table::WidthMode::Fixed(120.0)
    );
}

#[test]
fn a2_dropping_a_column_onto_itself_changes_and_reports_nothing() {
    vespertine_test_host::require_backend!();
    let mut harness = commit_harness();
    let x = divider(&mut harness, 1) + 40.0;
    harness
        .host
        .mouse_down(f64::from(x), f64::from(header_y()), 0, "")
        .unwrap();
    harness
        .host
        .mouse_move(f64::from(x - 3.0), f64::from(header_y()), Some(0), "")
        .unwrap();
    harness
        .host
        .mouse_move(f64::from(x + 3.0), f64::from(header_y()), Some(0), "")
        .unwrap();
    assert_eq!(harness.read(|state| state.insertion_indicator()), None);
    harness
        .host
        .mouse_up(f64::from(x + 3.0), f64::from(header_y()), 0, "")
        .unwrap();
    assert!(harness.events.borrow().is_empty());
    assert_eq!(
        harness.read(|state| state.column_order()),
        ["id", "msg", "author", "lines"]
    );
}

#[test]
fn r7_a_header_click_outside_a_handle_requests_the_sort_and_the_sorted_column_shows_an_indicator() {
    vespertine_test_host::require_backend!();
    let mut harness = commit_harness();
    let x = divider(&mut harness, 1) + 40.0;
    harness
        .host
        .click(f64::from(x), f64::from(header_y()), 0, "")
        .unwrap();
    assert_eq!(
        *harness.events.borrow(),
        [TableEvent::SortRequested("author".to_owned())]
    );
    harness.update(|state, cx| {
        state.set_sort(
            Some(Sort {
                column: "author".to_owned(),
                direction: SortDirection::Descending,
            }),
            cx,
        )
    });
    assert_eq!(
        harness.read(|state| state.sort().map(|s| s.direction)),
        Some(SortDirection::Descending)
    );
}

// ---- A9: header rule and numeric alignment -------------------------------------------------

/// The horizontal extent of pixels that differ from `background` inside a region (device px).
fn ink_extent(
    pixel: &dyn Fn(u32, u32) -> [u8; 4],
    x0: u32,
    x1: u32,
    y0: u32,
    y1: u32,
    background: [u8; 3],
) -> Option<(u32, u32)> {
    let mut extent: Option<(u32, u32)> = None;
    for y in y0..y1 {
        for x in x0..x1 {
            let color = pixel(x, y);
            if (0..3).any(|c| color[c].abs_diff(background[c]) > 40) {
                extent = Some(match extent {
                    Some((min, max)) => (min.min(x), max.max(x)),
                    None => (x, x),
                });
            }
        }
    }
    extent
}

fn one_row_harness(columns: Vec<Column>) -> Harness {
    let mut harness = Harness::new(
        600.0,
        200.0,
        columns,
        Some(36.0),
        |row, column| match column {
            "name" => Some(Cell::text(format!("row {row}"))),
            "lines" => Some(Cell::number(88)),
            _ => None,
        },
        |table| table.row_hover(false),
    );
    harness.update(|state, cx| state.set_data(rows(1), false, false, cx));
    harness
}

#[test]
fn a9_numeric_cells_end_align_unless_the_descriptor_says_otherwise() {
    vespertine_test_host::require_backend!();
    // The Lines column is 100 px wide at x 500..600; padding is 12 px.
    let measure = |columns: Vec<Column>| -> (u32, u32, f32) {
        let mut harness = one_row_harness(columns);
        let image = harness.host.screenshot_image().unwrap();
        let scale = image.width() as f32 / 600.0;
        let pixel = |x: u32, y: u32| image.get_pixel(x, y).0;
        // Row 0 spans y 32..68 (below the 32 px header); stay clear of the hairline.
        let (y0, y1) = ((36.0 * scale) as u32, (62.0 * scale) as u32);
        let (min, max) = ink_extent(
            &pixel,
            (500.0 * scale) as u32,
            (600.0 * scale) as u32,
            y0,
            y1,
            [0x10, 0x10, 0x10],
        )
        .expect("the number is painted");
        (min, max, scale)
    };
    let default = vec![
        Column::new("name", "Name"),
        Column::new("lines", "Lines").width(100.0),
    ];
    let (_, max, scale) = measure(default);
    let right_edge = max as f32 / scale;
    assert!(
        (right_edge - 588.0).abs() <= 3.0,
        "a number in a default-aligned column ends at the cell's end padding, got {right_edge}"
    );
    let explicit = vec![
        Column::new("name", "Name"),
        Column::new("lines", "Lines")
            .width(100.0)
            .align(Align::Start),
    ];
    let (min, _, scale) = measure(explicit);
    let left_edge = min as f32 / scale;
    assert!(
        (left_edge - 512.0).abs() <= 3.0,
        "an explicit start alignment wins, got {left_edge}"
    );
    let centered = vec![
        Column::new("name", "Name"),
        Column::new("lines", "Lines")
            .width(100.0)
            .align(Align::Center),
    ];
    let (min, max, scale) = measure(centered);
    let middle = (min + max) as f32 / 2.0 / scale;
    assert!((middle - 550.0).abs() <= 3.0, "centered, got {middle}");
}

#[test]
fn a9_the_header_row_is_omitted_when_every_label_is_empty() {
    vespertine_test_host::require_backend!();
    let with_header = one_row_harness(vec![
        Column::new("name", "Name"),
        Column::new("lines", "Lines").width(100.0),
    ]);
    let painted = with_header.host.painted_text();
    assert!(painted.iter().any(|text| text == "Name"));
    assert!(painted.iter().any(|text| text == "Lines"));
    let mut without = one_row_harness(vec![
        Column::new("name", ""),
        Column::new("lines", "").width(100.0),
    ]);
    let painted = without.host.painted_text();
    assert!(painted.iter().any(|text| text == "row 0"));
    assert!(!painted.iter().any(|text| text == "Name" || text.is_empty()));
    // Without a header the first row starts at the top: its number sits in the first 36 px.
    let image = without.host.screenshot_image().unwrap();
    let scale = image.width() as f32 / 600.0;
    let pixel = |x: u32, y: u32| image.get_pixel(x, y).0;
    let top = ink_extent(
        &pixel,
        0,
        (600.0 * scale) as u32,
        0,
        (30.0 * scale) as u32,
        [0x10, 0x10, 0x10],
    );
    assert!(top.is_some(), "row content starts at y = 0");
}

// ---- A7: alignment of the graph gutter with the rows ---------------------------------------

const TABLE_EVEN: u32 = 0xff0000;
const TABLE_ODD: u32 = 0x0000ff;
const GUTTER_EVEN: u32 = 0x00ff00;
const GUTTER_ODD: u32 = 0xffff00;

fn classify(pixel: [u8; 4], even: u32, odd: u32) -> Option<usize> {
    let near = |hex: u32| {
        let (r, g, b) = (
            ((hex >> 16) & 255) as u8,
            ((hex >> 8) & 255) as u8,
            (hex & 255) as u8,
        );
        pixel[0].abs_diff(r) < 24 && pixel[1].abs_diff(g) < 24 && pixel[2].abs_diff(b) < 24
    };
    if near(even) {
        Some(0)
    } else if near(odd) {
        Some(1)
    } else {
        None
    }
}

fn striped_harness() -> Harness {
    let mut harness = Harness::new(
        600.0,
        300.0,
        vec![
            Column::new("msg", "Message"),
            Column::new("author", "Author").width(120.0),
        ],
        Some(36.0),
        |row, column| {
            let n: usize = row.parse().unwrap();
            match column {
                "msg" => Some(Cell::element(div().absolute().inset_0().bg(rgb(
                    if n.is_multiple_of(2) {
                        TABLE_EVEN
                    } else {
                        TABLE_ODD
                    },
                )))),
                "author" => Some(Cell::text("ada")),
                _ => None,
            }
        },
        |table| {
            table
                .row_hover(false)
                .graph(GraphColumn::new(2, |gutter, window, _| {
                    for ix in gutter.rows.clone() {
                        let color = if ix % 2 == 0 { GUTTER_EVEN } else { GUTTER_ODD };
                        let top = gutter.row_top(ix);
                        let bounds = Bounds::new(
                            point(gutter.bounds.left() + px(4.0), top),
                            size(px(16.0), gutter.row_height),
                        );
                        window.paint_quad(fill(bounds, rgb(color)));
                    }
                }))
        },
    );
    harness.update(|state, cx| state.set_data(rows(60), false, false, cx));
    harness
}

/// Checks that the gutter stripes switch exactly where the row stripes do, over the body.
fn assert_gutter_matches_rows(harness: &mut Harness, label: &str) {
    let image = harness.host.screenshot_image().unwrap();
    let scale = image.width() as f32 / 600.0;
    let pixel = |x: u32, y: u32| image.get_pixel(x, y).0;
    let gutter_x = (10.0 * scale) as u32;
    let table_x = (300.0 * scale) as u32;
    let body_top = (32.0 * scale) as u32;
    let bottom = (300.0 * scale) as u32;
    let mut checked = 0;
    let mut last_parity: Option<usize> = None;
    let mut transitions = 0;
    for y in body_top..bottom {
        let table = classify(pixel(table_x, y), TABLE_EVEN, TABLE_ODD);
        let gutter = classify(pixel(gutter_x, y), GUTTER_EVEN, GUTTER_ODD);
        match table {
            Some(parity) => {
                assert_eq!(
                    gutter,
                    Some(parity),
                    "{label}: row boundary mismatch at device y {y}"
                );
                if last_parity.is_some_and(|last| last != parity) {
                    transitions += 1;
                }
                last_parity = Some(parity);
                checked += 1;
            }
            None => {
                // The hairline at the bottom of a row: the gutter has not switched yet.
                if let Some(last) = last_parity {
                    assert_eq!(
                        gutter,
                        Some(last),
                        "{label}: gutter switched inside the hairline at device y {y}"
                    );
                }
            }
        }
    }
    assert!(
        checked > 100 && transitions >= 5,
        "{label}: {checked} checked pixels, {transitions} boundaries"
    );
}

#[test]
fn a7_the_gutters_row_boundaries_equal_the_table_rows_after_scrolling_and_resizing() {
    vespertine_test_host::require_backend!();
    let mut harness = striped_harness();
    assert_gutter_matches_rows(&mut harness, "unscrolled");
    // Scroll by a distance that is not a multiple of the 36 px row height.
    harness
        .host
        .scroll_wheel(300.0, 150.0, 0.0, -50.0, "")
        .unwrap();
    harness.host.flush();
    let offset = harness.read(|state| {
        let top = state.list_state().logical_scroll_top();
        top.item_ix as f32 * 36.0 + f32::from(top.offset_in_item)
    });
    assert!((offset - 50.0).abs() < 0.5, "scrolled by {offset}px");
    assert_gutter_matches_rows(&mut harness, "scrolled by 50px");
    harness
        .host
        .scroll_wheel(300.0, 150.0, 0.0, -17.5, "")
        .unwrap();
    harness.host.flush();
    assert_gutter_matches_rows(&mut harness, "scrolled by 67.5px");
    // Resizing columns leaves the row heights, and so the alignment, unchanged.
    let x = harness.read(|state| {
        state
            .gutter_geometry()
            .map_or(0.0, |geometry| geometry.width)
            + state.rendered_widths()[0]
    });
    harness.host.mouse_down(f64::from(x), 16.0, 0, "").unwrap();
    harness
        .host
        .mouse_move(f64::from(x - 50.0), 16.0, Some(0), "")
        .unwrap();
    harness
        .host
        .mouse_up(f64::from(x - 50.0), 16.0, 0, "")
        .unwrap();
    assert_eq!(
        harness.read(|state| state.widths().get("author").copied()),
        Some(170.0)
    );
    assert_gutter_matches_rows(&mut harness, "after resizing");
    // Reordering does not move rows either.
    harness.update(|state, cx| {
        state.drag_column_over("author", 10.0, cx);
        state.commit_reorder(cx);
    });
    assert_gutter_matches_rows(&mut harness, "after reordering");
}

// ---- A5 through the state: groups and pages --------------------------------------------------

#[test]
fn a5_collapsing_a_group_and_appending_a_page_keep_the_anchor_row_in_place() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(
        600.0,
        300.0,
        commit_columns(),
        Some(36.0),
        text_cells,
        |table| table,
    );
    let group = |key: &str, range: std::ops::Range<usize>| {
        GroupSpec::new(key, range.map(|n| TableRow::item(&n.to_string())).collect())
    };
    harness.update(|state, cx| {
        state.set_data(
            vec![group("may", 0..20), group("apr", 20..40)],
            true,
            false,
            cx,
        )
    });
    // Scroll so that item 25 is the first visible row at 11 px.
    let ix = harness.read(|state| {
        state
            .rows()
            .iter()
            .position(|row| row.id == "item:25")
            .unwrap()
    });
    harness.read(|state| {
        state.list_state().scroll_to(vespertine::ListOffset {
            item_ix: ix,
            offset_in_item: px(11.0),
        })
    });
    harness.settle();
    let anchor = harness.read(|state| state.anchor()).unwrap();
    assert_eq!(
        (anchor.id.as_str(), f32::from(anchor.offset)),
        ("item:25", 11.0)
    );
    // Collapse an earlier group.
    harness.update(|state, cx| state.toggle_group("may", false, cx));
    let anchor = harness.read(|state| state.anchor()).unwrap();
    assert_eq!(
        (anchor.id.as_str(), f32::from(anchor.offset)),
        ("item:25", 11.0)
    );
    assert!(
        harness
            .events
            .borrow()
            .contains(&TableEvent::GroupToggled("may".to_owned()))
    );
    // Append a page.
    harness.update(|state, cx| {
        state.set_data(
            vec![group("may", 0..20), group("apr", 20..60)],
            true,
            false,
            cx,
        )
    });
    let anchor = harness.read(|state| state.anchor()).unwrap();
    assert_eq!(
        (anchor.id.as_str(), f32::from(anchor.offset)),
        ("item:25", 11.0)
    );
    // The footer is a row of the list; filter text changes clear the collapsed set.
    assert_eq!(
        harness
            .read(|state| state.rows().last().map(|row| row.id.clone()))
            .as_deref(),
        Some("footer:more")
    );
    harness.update(|state, cx| state.set_filter_text("fix", cx));
    assert!(harness.read(|state| state.collapsed().is_empty()));
    assert!(
        harness
            .host
            .painted_text()
            .iter()
            .any(|text| text.starts_with("message "))
    );
}

#[test]
fn the_footer_requests_the_next_page_once_and_shows_loading_meanwhile() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(
        600.0,
        600.0,
        commit_columns(),
        Some(36.0),
        text_cells,
        |t| t,
    );
    harness.update(|state, cx| state.set_data(rows(3), true, false, cx));
    assert!(
        harness
            .host
            .painted_text()
            .iter()
            .any(|text| text == "Load more")
    );
    // The footer is the fourth row: y = 32 (header) + 3 x 36 + 18.
    harness
        .host
        .click(300.0, 32.0 + 3.0 * 36.0 + 18.0, 0, "")
        .unwrap();
    assert_eq!(*harness.events.borrow(), [TableEvent::LoadMoreRequested]);
    harness.update(|state, cx| state.set_loading_more(true, cx));
    assert!(
        harness
            .host
            .painted_text()
            .iter()
            .any(|text| text == "Loading…")
    );
    harness
        .host
        .click(300.0, 32.0 + 3.0 * 36.0 + 18.0, 0, "")
        .unwrap();
    assert_eq!(
        harness.events.borrow().len(),
        1,
        "no second request while loading"
    );
    harness.update(|state, cx| state.set_data(rows(3), false, false, cx));
    assert!(
        !harness
            .host
            .painted_text()
            .iter()
            .any(|text| text == "Loading…")
    );
}

#[test]
fn clicking_a_group_heading_toggles_it() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(
        600.0,
        400.0,
        commit_columns(),
        Some(36.0),
        text_cells,
        |t| t,
    );
    harness.update(|state, cx| {
        state.set_data(
            vec![GroupSpec::new(
                "may",
                vec![TableRow::item("1"), TableRow::item("2")],
            )],
            false,
            false,
            cx,
        )
    });
    assert_eq!(harness.read(|state| state.rows().len()), 3);
    harness.host.click(100.0, 32.0 + 18.0, 0, "").unwrap();
    assert_eq!(harness.read(|state| state.rows().len()), 1);
    assert_eq!(
        *harness.events.borrow(),
        [TableEvent::GroupToggled("may".to_owned())]
    );
    harness.host.click(100.0, 32.0 + 18.0, 0, "").unwrap();
    assert_eq!(harness.read(|state| state.rows().len()), 3);
}

#[test]
fn rows_select_and_missing_cells_render_empty() {
    vespertine_test_host::require_backend!();
    let clicked = Rc::new(RefCell::new(Vec::<String>::new()));
    let sink = clicked.clone();
    let mut harness = Harness::new(
        600.0,
        300.0,
        commit_columns(),
        Some(36.0),
        |row, column| (column == "msg").then(|| Cell::text(format!("message {row}"))),
        move |table| {
            let sink = sink.clone();
            table
                .selected(Some("1".to_owned()))
                .on_row_click(move |id, _, _| sink.borrow_mut().push(id.to_owned()))
        },
    );
    harness.update(|state, cx| state.set_data(rows(3), false, false, cx));
    harness
        .host
        .click(300.0, 32.0 + 36.0 + 18.0, 0, "")
        .unwrap();
    assert_eq!(*clicked.borrow(), ["1"]);
    let painted = harness.host.painted_text();
    assert!(painted.iter().any(|text| text == "message 2"));
    assert!(
        !painted.iter().any(|text| text == "ada"),
        "missing cells render empty"
    );
}
