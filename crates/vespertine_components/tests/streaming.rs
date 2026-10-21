//! CMP-029: the follow controller over a real virtual list (A1, A4, A5, A6, A7 navigation) and the
//! history pixel tests (A9, A10).

use std::{cell::Cell, cell::RefCell, rc::Rc};

use vespertine::{
    AppContext as _, Context, IntoElement, ListAlignment, ListOffset, ListState, ParentElement,
    Render, Styled, Window, div, list, px, rgb,
};
use vespertine_components::streaming::{
    Commit, Direction, FollowController, History, HistoryRow, ReaderSample,
    history::EDGE_ROW_HEIGHT,
};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

#[path = "support/sample_service.rs"]
mod sample_service;
use sample_service::SampleService;

const VIEWPORT: f32 = 300.0;

struct Chat {
    list: ListState,
    heights: Rc<RefCell<Vec<f32>>>,
    controller: FollowController,
}

impl Render for Chat {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let heights = self.heights.clone();
        let height = self.controller.list_height(VIEWPORT);
        div().size_full().bg(rgb(0x101010)).child(
            list(self.list.clone(), move |ix, _, _| {
                let h = heights.borrow().get(ix).copied().unwrap_or(10.0);
                div()
                    .h(px(h))
                    .w_full()
                    .bg(rgb(if ix % 2 == 0 { 0x303030 } else { 0x404040 }))
                    .into_any_element()
            })
            .w(px(300.0))
            .h(px(height)),
        )
    }
}

struct Fixture {
    host: TestHost,
    list: ListState,
    heights: Rc<RefCell<Vec<f32>>>,
    controller: FollowController,
    now: f64,
    user_events: Rc<Cell<u32>>,
}

fn fixture(rows: Vec<f32>) -> Fixture {
    let list = ListState::new(rows.len(), ListAlignment::Top, px(100.0));
    let heights = Rc::new(RefCell::new(rows));
    let controller = FollowController::attach(list.clone());
    let user_events = Rc::new(Cell::new(0));
    let counter = user_events.clone();
    controller.on_visible_range(move |event, _, _| {
        if event.user_initiated {
            counter.set(counter.get() + 1);
        }
    });
    let (l, h, c) = (list.clone(), heights.clone(), controller.clone());
    let mut host = TestHost::with_view(
        HostOptions::with_size(300.0, VIEWPORT as f64),
        move |_, cx| {
            cx.new(|_| Chat {
                list: l,
                heights: h,
                controller: c,
            })
        },
    )
    .expect("host");
    host.motion_set(1000.0).unwrap();
    host.flush();
    Fixture {
        host,
        list,
        heights,
        controller,
        now: 1000.0,
        user_events,
    }
}

impl Fixture {
    /// Runs `frames` frames 16 ms apart. The host's effect loop runs the next-frame callbacks
    /// (the stepper) before it draws, as a live window's frame callback does.
    fn frames(&mut self, frames: usize) {
        for _ in 0..frames {
            self.now += 1000.0 / 60.0;
            self.host.motion_set(self.now).unwrap();
            self.host.paint_frame();
        }
    }

    fn with_window<R>(
        &mut self,
        f: impl FnOnce(&mut Window, &mut vespertine::App) -> R + 'static,
    ) -> R
    where
        R: 'static,
    {
        let handle = self.host.window();
        self.host
            .app()
            .update_window(handle, move |_, window, cx| f(window, cx))
            .expect("window")
    }

    fn commit(&mut self, commit: Commit) {
        let controller = self.controller.clone();
        self.with_window(move |window, cx| controller.commit(commit, window, cx));
    }

    fn append(&mut self, height: f32) {
        let n = self.heights.borrow().len();
        self.heights.borrow_mut().push(height);
        self.commit(Commit::Splice {
            old_range: n..n,
            new_count: 1,
        });
        self.frames(1);
    }

    fn grow_tail(&mut self, delta: f32) {
        let n = self.heights.borrow().len();
        *self.heights.borrow_mut().last_mut().unwrap() += delta;
        self.commit(Commit::ReplaceInPlace(n - 1..n));
        self.frames(1);
    }

    fn wheel(&mut self, dy: f32) {
        self.host
            .scroll_wheel(150.0, 150.0, 0.0, dy as f64, "")
            .unwrap();
        self.frames(1);
    }

    fn distance(&self) -> f32 {
        self.controller.distance_from_bottom()
    }

    fn top_of(&self, row: usize) -> f32 {
        f32::from(self.list.bounds_for_item(row).expect("row bounds").top())
    }
}

fn uniform(count: usize, height: f32) -> Vec<f32> {
    vec![height; count]
}

#[test]
fn a1_a_streaming_tail_stays_glued_until_the_user_detaches_and_the_anchor_never_moves() {
    require_backend!();
    let mut f = fixture(uniform(30, 40.0));
    f.frames(2);
    assert!(f.controller.is_pinned());
    assert!(
        f.distance() <= 0.5,
        "opens at the bottom, D = {}",
        f.distance()
    );

    // Streaming: the tail grows while pinned, the end stays in view.
    f.controller.set_tail_streaming(true);
    for _ in 0..10 {
        f.grow_tail(7.0);
        assert!(f.distance() <= 2.0, "D = {}", f.distance());
    }
    assert!(f.controller.is_pinned());

    // An upward wheel notch detaches.
    f.wheel(30.0);
    assert!(!f.controller.is_pinned());
    assert!(
        f.user_events.get() >= 1,
        "the wheel reached the user handler"
    );
    let anchor_row = f.list.logical_scroll_top().item_ix;
    let before = f.top_of(anchor_row);

    // Tall media appended below, tokens streaming: the detached anchor does not move.
    f.append(500.0);
    f.append(500.0);
    f.grow_tail(80.0);
    f.frames(5);
    let after = f.top_of(anchor_row);
    assert!(
        (after - before).abs() <= 1.0,
        "the detached anchor moved from {before} to {after}"
    );
    assert!(!f.controller.is_pinned());

    // Further up, then a downward gesture to within 70 px while moving toward the bottom re-pins.
    f.wheel(100.0);
    assert!(!f.controller.is_pinned());
    let far = f.distance();
    assert!(far > 100.0);
    f.wheel(-(far - 60.0));
    assert!(
        f.controller.is_pinned(),
        "re-pinned at D = {}",
        f.distance()
    );
}

#[test]
fn a1_a_wheel_up_notch_inside_the_band_does_not_re_pin() {
    require_backend!();
    let mut f = fixture(uniform(30, 40.0));
    f.frames(2);
    f.wheel(30.0);
    assert!(!f.controller.is_pinned());
    let d = f.distance();
    assert!(d > 2.0 && d < 70.0, "inside the band, D = {d}");
    // Another notch up: moving away, no re-pin.
    f.wheel(20.0);
    assert!(!f.controller.is_pinned());
    assert!(f.distance() > d);
}

#[test]
fn a4_a_bottom_dock_opening_while_pinned_keeps_the_end_above_the_reservation() {
    require_backend!();
    let mut f = fixture(uniform(30, 40.0));
    f.frames(2);
    assert!(f.distance() <= 0.5);
    let controller = f.controller.clone();
    let reserved =
        f.with_window(move |window, _| controller.set_bottom_clearance(120.0, 300.0, window));
    assert_eq!(reserved, 120.0);
    f.frames(3);
    // The list is now 180 px tall and its end rests at its bottom.
    let viewport = f.list.viewport_bounds();
    assert!((f32::from(viewport.size.height) - 180.0).abs() < 1.0);
    let last = f.list.bounds_for_item(29).expect("last row is laid out");
    assert!(
        (f32::from(last.bottom() - viewport.bottom())).abs() <= 1.0,
        "the end rests above the reservation"
    );
    assert!(
        f.controller.is_idle(),
        "no more frames are requested once settled"
    );
    // The reservation clamps to its limit.
    let controller = f.controller.clone();
    let reserved =
        f.with_window(move |window, _| controller.set_bottom_clearance(900.0, 250.0, window));
    assert_eq!(reserved, 250.0);
    // Sub-pixel jitter is ignored.
    let controller = f.controller.clone();
    let reserved =
        f.with_window(move |window, _| controller.set_bottom_clearance(250.3, 250.0, window));
    assert_eq!(reserved, 250.0);
}

#[test]
fn a5_the_jump_button_persists_until_the_end_and_a_far_jump_first_teleports() {
    require_backend!();
    let mut f = fixture(uniform(100, 40.0));
    f.frames(2);
    // Detach far above the end (3700 px).
    f.wheel(3700.0);
    assert!(!f.controller.is_pinned());
    assert!(f.distance() > 320.0);
    assert!(f.controller.show_jump_button());
    // Scrolling down keeps it until the end is within 2 px.
    f.wheel(-1000.0);
    assert!(
        f.controller.show_jump_button(),
        "hysteresis at D = {}",
        f.distance()
    );

    // From ten viewports away the jump first moves by D - 2.5 viewports.
    let mut g = fixture(uniform(100, 40.0));
    g.frames(2);
    g.wheel(10.0 * VIEWPORT);
    let d = g.distance();
    assert!((d - 10.0 * VIEWPORT).abs() < 1.5, "D = {d}");
    let controller = g.controller.clone();
    g.with_window(move |window, cx| controller.jump_to_bottom(window, cx));
    let teleported = g.distance();
    // The host's effect loop runs the frame the jump requested, as a live window does, so the
    // glide has taken at most its first step by now; the teleport itself moved D - 2.5 viewports.
    assert!(
        teleported <= 2.5 * VIEWPORT + 1.5 && teleported >= 2.3 * VIEWPORT,
        "first moved D - 2.5 viewports, left {teleported}"
    );
    assert!(g.controller.is_pinned());
    // Then the spring glides the rest and lands.
    g.frames(240);
    assert!(g.distance() <= 0.5, "landed, D = {}", g.distance());
    assert!(!g.controller.show_jump_button());
}

#[test]
fn a6_programmatic_scrolls_never_reach_the_user_handler_or_the_pin() {
    require_backend!();
    let mut f = fixture(uniform(60, 40.0));
    f.frames(2);
    assert!(f.controller.is_pinned());
    let before = f.user_events.get();
    // Programmatic scroll-by and scroll-to while pinned: no user event, the pin stays.
    f.list.scroll_by(px(-300.0));
    f.frames(1);
    f.list.scroll_to(ListOffset {
        item_ix: 3,
        offset_in_item: px(5.0),
    });
    f.frames(1);
    assert_eq!(f.user_events.get(), before, "no user events");
    assert!(
        f.controller.is_pinned(),
        "the pin survived programmatic scrolling"
    );

    // Detached: reaching the bottom programmatically does not re-pin.
    f.wheel(500.0);
    assert!(!f.controller.is_pinned());
    let count = f.user_events.get();
    f.list.scroll_to_end();
    f.frames(2);
    assert_eq!(f.user_events.get(), count);
    assert!(!f.controller.is_pinned());

    // Selection autoscroll claims the viewport and never re-pins, even at the end.
    let controller = f.controller.clone();
    let bounds = f.list.viewport_bounds();
    let y = f32::from(bounds.bottom());
    let step = f.with_window(move |window, _| controller.selection_autoscroll_tick(y, window));
    assert_eq!(step, 24.0);
    assert!(!f.controller.is_pinned());
    assert_eq!(f.user_events.get(), count);
}

#[test]
fn a7_navigation_reaches_a_row_that_was_never_measured_and_the_pin_is_released() {
    require_backend!();
    let mut f = fixture(uniform(1000, 40.0));
    f.frames(2);
    assert!(f.controller.is_pinned());
    let controller = f.controller.clone();
    f.with_window(move |window, cx| controller.scroll_to_row(600, window, cx));
    assert!(!f.controller.is_pinned(), "navigation claims the viewport");
    f.frames(60);
    assert_eq!(f.list.logical_scroll_top().item_ix, 600);
    assert!(f.list.logical_scroll_top().offset_in_item.abs() < px(1.0));
    assert!(!f.controller.is_pinned());
}

#[test]
fn a12_an_anchored_send_rests_the_item_at_its_inset_and_hands_off_when_filled() {
    require_backend!();
    let mut f = fixture(uniform(30, 40.0));
    f.frames(2);
    // A new user message arrives; the turn is anchored with a 20 px inset.
    f.append(60.0);
    let controller = f.controller.clone();
    f.with_window(move |window, cx| controller.begin_anchored_send(30, 20.0, window, cx));
    f.frames(120);
    let top = f.top_of(30) - f32::from(f.list.viewport_bounds().top());
    assert!(
        (top - 20.0).abs() <= 2.5,
        "the item rests at its inset, top = {top}"
    );
    assert!(f.controller.has_hold());
    assert!(!f.controller.is_pinned());
    // Following content fills the reservation: handoff to the pin.
    for _ in 0..8 {
        f.append(60.0);
        f.frames(2);
    }
    f.frames(120);
    assert!(
        f.controller.is_pinned(),
        "the pin engages once the reservation is filled"
    );
    assert!(f.distance() <= 2.0);
}

#[test]
fn a_stepper_glides_a_not_quite_live_tail_to_the_end_and_goes_idle() {
    require_backend!();
    let mut f = fixture(uniform(30, 40.0));
    f.frames(2);
    // Not streaming: a commit while pinned glides.
    for _ in 0..3 {
        f.append(120.0);
    }
    f.frames(240);
    assert!(f.distance() <= 0.5, "D = {}", f.distance());
    assert!(f.controller.is_idle());
}

// ---------------------------------------------------------------------------------------------
// History pixel tests.
// ---------------------------------------------------------------------------------------------

struct HistoryView {
    height: f32,
    list: ListState,
    rows: Rc<RefCell<Vec<HistoryRow<usize>>>>,
    service: SampleService,
}

impl Render for HistoryView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let service = self.service;
        div().size_full().bg(rgb(0x101010)).child(
            list(self.list.clone(), move |ix, _, _| {
                let height = match rows.borrow().get(ix) {
                    Some(HistoryRow::LeadingEdge | HistoryRow::TrailingEdge) | None => {
                        EDGE_ROW_HEIGHT
                    }
                    Some(HistoryRow::Item(id)) => service.height(*id),
                };
                div().h(px(height)).w_full().into_any_element()
            })
            .w(px(300.0))
            .h(px(self.height)),
        )
    }
}

struct HistoryFixture {
    host: TestHost,
    list: ListState,
    rows: Rc<RefCell<Vec<HistoryRow<usize>>>>,
    history: History<usize>,
    service: SampleService,
}

fn history_fixture(target: Option<usize>, viewport: f64) -> HistoryFixture {
    let service = SampleService::new(11);
    let history = History::mount(12, service.initial_page(12, target));
    let rows = Rc::new(RefCell::new(history.rows()));
    let list = ListState::new(history.row_count(), ListAlignment::Top, px(50.0));
    let (l, r) = (list.clone(), rows.clone());
    let mut host = TestHost::with_view(HostOptions::with_size(300.0, viewport), move |_, cx| {
        cx.new(|_| HistoryView {
            height: viewport as f32,
            list: l,
            rows: r,
            service,
        })
    })
    .expect("host");
    host.flush();
    HistoryFixture {
        host,
        list,
        rows,
        history,
        service,
    }
}

impl HistoryFixture {
    fn frame(&mut self) {
        self.host.paint_frame();
        self.host.paint_frame();
    }

    fn land(&mut self, direction: Direction) -> Option<vespertine_components::streaming::Restore> {
        let cursor = match direction {
            Direction::Previous => self.history.items().next().copied(),
            _ => self.history.items().last().copied(),
        };
        let request = vespertine_components::streaming::Request { direction, cursor };
        let page = self.service.fetch_page(12, &request);
        let sample = ReaderSample::from_list(&self.list);
        let landing = self.history.apply_page(direction, page, sample);
        *self.rows.borrow_mut() = self.history.rows();
        self.history.apply_to_list(&self.list, &landing);
        self.frame();
        landing.restore
    }
}

#[test]
fn a9_an_older_page_landing_in_the_leading_edge_row_keeps_the_old_first_item_in_place() {
    require_backend!();
    let mut f = history_fixture(None, 500.0);
    f.frame();
    // The reader waits 40 px into the leading edge row.
    f.list.scroll_to(ListOffset {
        item_ix: 0,
        offset_in_item: px(40.0),
    });
    f.frame();
    let old_first = f.history.row_of(&388).unwrap();
    let before = f32::from(f.list.bounds_for_item(old_first).unwrap().top());
    let restore = f.land(Direction::Previous).expect("a restore is forced");
    assert_eq!(restore.offset, -120.0);
    let new_row = f.history.row_of(&388).unwrap();
    assert_eq!(new_row, 1 + 12);
    let after = f32::from(f.list.bounds_for_item(new_row).unwrap().top());
    assert!(
        (after - before).abs() <= 1.0,
        "the old first item moved from {before} to {after}"
    );
}

#[test]
fn a9_a_newer_page_takes_the_empty_trailing_row_s_place_without_moving_text() {
    require_backend!();
    for (viewport, expected_offset) in [(500.0, -340.0), (100.0, 60.0)] {
        let mut f = history_fixture(Some(200), viewport);
        f.frame();
        // Scroll to the end sentinel: the viewport bottom sits on the trailing row's bottom.
        f.list.scroll_to_end();
        f.frame();
        let trailing = f.history.row_count() - 1;
        let trailing_top = f32::from(f.list.bounds_for_item(trailing).map_or_else(
            || {
                // The trailing row is measured; its top follows from the viewport bottom.
                f.list.viewport_bounds().bottom() - px(EDGE_ROW_HEIGHT)
            },
            |bounds| bounds.top(),
        ));
        let restore = f.land(Direction::Next).expect("restore");
        assert_eq!(restore.offset, expected_offset, "viewport {viewport}");
        let first_appended = f.history.row_of(&206).unwrap();
        let top = f32::from(f.list.bounds_for_item(first_appended).unwrap().top());
        assert!(
            (top - trailing_top).abs() <= 1.0,
            "viewport {viewport}: the first appended item's top {top} equals the old trailing row's top {trailing_top}"
        );
    }
}

#[test]
fn a9_a_retained_item_anchor_needs_no_forced_scroll_and_pages_stay_bounded() {
    require_backend!();
    let mut f = history_fixture(None, 500.0);
    f.frame();
    // Anchored on a retained item (row 4): nothing is forced and the item stays put.
    f.list.scroll_to(ListOffset {
        item_ix: 4,
        offset_in_item: px(7.0),
    });
    f.frame();
    let item = *f.history.items().nth(3).unwrap();
    let row = f.history.row_of(&item).unwrap();
    let before = f32::from(f.list.bounds_for_item(row).unwrap().top());
    assert_eq!(f.land(Direction::Previous), None);
    let after = f32::from(
        f.list
            .bounds_for_item(f.history.row_of(&item).unwrap())
            .unwrap()
            .top(),
    );
    assert!((after - before).abs() <= 1.0, "{before} -> {after}");
    for _ in 0..7 {
        f.land(Direction::Previous);
        assert!(f.history.page_count() <= 5);
    }
}
