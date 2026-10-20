//! CMP-021 R20: the terminal panel draws its tab model as the closable, reorderable tab strip
//! of CMP-017 and keeps the two in step (select, close, reorder, new tab, titles, exit state).

use std::{cell::RefCell, rc::Rc};
use vespertine::{
    AppContext, Bounds, Context, Entity, IntoElement, Modifiers, MouseButton, ParentElement,
    Pixels, Point, Render, Styled, TestAppContext, VisualTestContext, Window, div, point, px,
};
use vespertine_components::tabs::TabStrip;
use vespertine_components::terminal::{TabId, TerminalPanel, exit_status_line};

type Closed = Rc<RefCell<Vec<(TabId, usize)>>>;

struct Root {
    panel: Entity<TerminalPanel>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.panel.clone())
    }
}

struct Fixture {
    panel: Entity<TerminalPanel>,
    tabs: Vec<TabId>,
    closed: Closed,
    new_tab_requests: Rc<RefCell<usize>>,
}

fn setup(
    cx: &mut TestAppContext,
    count: usize,
    host_opens_tabs: bool,
) -> (Fixture, &mut VisualTestContext) {
    let closed: Closed = Rc::default();
    let requests = Rc::new(RefCell::new(0));
    let (root, vcx) = cx.add_window_view({
        let closed = closed.clone();
        let requests = requests.clone();
        move |_, cx| {
            let panel = cx.new(|cx| {
                let mut panel = TerminalPanel::new(cx);
                panel.on_tab_closed(move |tab, remaining, _, _| {
                    closed.borrow_mut().push((tab, remaining));
                });
                if host_opens_tabs {
                    panel.on_new_tab(move |_, _| *requests.borrow_mut() += 1);
                }
                for _ in 0..count {
                    panel.open_tab(cx);
                }
                panel
            });
            Root { panel }
        }
    });
    vcx.update(|window, _| window.activate_window());
    let panel = root.read_with(vcx, |root, _| root.panel.clone());
    let tabs = panel.read_with(vcx, |panel, _| panel.tab_ids());
    redraw(vcx);
    redraw(vcx);
    (
        Fixture {
            panel,
            tabs,
            closed,
            new_tab_requests: requests,
        },
        vcx,
    )
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn bounds(cx: &mut VisualTestContext, selector: String) -> Bounds<Pixels> {
    cx.debug_bounds(&selector)
        .unwrap_or_else(|| panic!("no element {selector}"))
}

fn chip(cx: &mut VisualTestContext, tab: TabId) -> Point<Pixels> {
    bounds(cx, format!("tab:{}", tab.0)).center()
}

fn click(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_mouse_move(position, None, Modifiers::default());
    redraw(cx);
    cx.simulate_click(position, Modifiers::default());
    redraw(cx);
}

fn order(fixture: &Fixture, cx: &mut VisualTestContext) -> Vec<TabId> {
    fixture.panel.read_with(cx, |panel, _| panel.tab_ids())
}

fn active(fixture: &Fixture, cx: &mut VisualTestContext) -> usize {
    fixture.panel.read_with(cx, |panel, _| panel.active_index())
}

fn strip(fixture: &Fixture, cx: &mut VisualTestContext) -> Entity<TabStrip> {
    fixture
        .panel
        .read_with(cx, |panel, _| panel.tab_strip().clone())
}

/// `(ids in strip order, selected id)` as the strip itself reports them.
fn strip_state(fixture: &Fixture, cx: &mut VisualTestContext) -> (Vec<String>, Option<String>) {
    let strip = strip(fixture, cx);
    strip.read_with(cx, |strip, _| {
        (
            strip.specs().iter().map(|tab| tab.id.to_string()).collect(),
            strip.selected_id().map(ToString::to_string),
        )
    })
}

fn ids(tabs: &[TabId]) -> Vec<String> {
    tabs.iter().map(|tab| tab.0.to_string()).collect()
}

#[vespertine::test]
fn the_strip_lists_the_panels_tabs_with_the_active_one_selected(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, 3, true);
    for tab in &fixture.tabs {
        bounds(vcx, format!("tab:{}", tab.0));
    }
    let (listed, selected) = strip_state(&fixture, vcx);
    assert_eq!(listed, ids(&fixture.tabs));
    assert_eq!(
        selected,
        Some(fixture.tabs[2].0.to_string()),
        "a new tab is active"
    );

    // A tab opened by the host appears in the strip and takes the selection.
    let fourth = fixture.panel.update(vcx, |panel, cx| panel.open_tab(cx));
    redraw(vcx);
    bounds(vcx, format!("tab:{}", fourth.0));
    assert_eq!(strip_state(&fixture, vcx).1, Some(fourth.0.to_string()));
}

#[vespertine::test]
fn titles_and_exit_state_reach_the_chips(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, 2, true);
    let (first, second) = (fixture.tabs[0], fixture.tabs[1]);
    let strip = strip(&fixture, vcx);
    let label = |vcx: &mut VisualTestContext, tab: TabId| {
        strip.read_with(vcx, |strip, _| {
            let spec = strip
                .specs()
                .iter()
                .find(|spec| spec.id.as_str() == tab.0.to_string())
                .expect("listed");
            (spec.title.to_string(), spec.label().to_string())
        })
    };
    assert_eq!(
        label(vcx, first).0,
        "Terminal 1",
        "untitled tabs are numbered"
    );
    fixture.panel.update(vcx, |panel, cx| {
        panel.feed(first, b"\x1b]0;build\x07", cx);
    });
    assert_eq!(
        label(vcx, first).0,
        "build",
        "the program-set title is shown"
    );
    fixture.panel.update(vcx, |panel, cx| {
        panel.feed(first, b"\x1b]2;\x07", cx);
    });
    assert_eq!(
        label(vcx, first).0,
        "Terminal 1",
        "a reset title falls back"
    );

    fixture.panel.update(vcx, |panel, cx| {
        panel.mark_exited(second, 3, cx);
        panel.feed(second, &exit_status_line(3), cx);
    });
    assert_eq!(
        label(vcx, second).1,
        "Terminal 2 (exited with status 3)",
        "the exit status is in the accessible name"
    );
}

#[vespertine::test]
fn clicking_a_chip_selects_that_tab_and_returns_focus_to_the_grid(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, 3, true);
    let position = chip(vcx, fixture.tabs[0]);
    click(vcx, position);
    assert_eq!(active(&fixture, vcx), 0);
    assert_eq!(
        fixture.panel.read_with(vcx, |panel, _| panel.active_tab()),
        Some(fixture.tabs[0])
    );
    assert_eq!(
        strip_state(&fixture, vcx).1,
        Some(fixture.tabs[0].0.to_string())
    );
    let focused = fixture.panel.update_in(vcx, |panel, window, cx| {
        vespertine::Focusable::focus_handle(panel, cx).is_focused(window)
    });
    assert!(focused, "typing continues in the terminal");
}

#[vespertine::test]
fn the_close_slot_closes_a_tab_by_the_active_index_rule(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, 4, true);
    let [a, b, c, d] = [
        fixture.tabs[0],
        fixture.tabs[1],
        fixture.tabs[2],
        fixture.tabs[3],
    ];
    // Active is `d` (index 3). Closing `a` (index 0 < 3) moves the active index down by one.
    let slot = bounds(vcx, format!("tab-close:{}", a.0)).center();
    click(vcx, slot);
    assert_eq!(order(&fixture, vcx), vec![b, c, d]);
    assert_eq!(active(&fixture, vcx), 2);
    assert_eq!(strip_state(&fixture, vcx).1, Some(d.0.to_string()));
    assert_eq!(fixture.closed.borrow().as_slice(), [(a, 3)]);

    // Closing the active last tab keeps the index, clamped to the last tab.
    let slot = bounds(vcx, format!("tab-close:{}", d.0)).center();
    click(vcx, slot);
    assert_eq!(order(&fixture, vcx), vec![b, c]);
    assert_eq!(active(&fixture, vcx), 1);
    assert_eq!(strip_state(&fixture, vcx).1, Some(c.0.to_string()));

    // The last tabs close down to an empty strip; the host is told how many are left.
    let slot = bounds(vcx, format!("tab-close:{}", c.0)).center();
    click(vcx, slot);
    let slot = bounds(vcx, format!("tab-close:{}", b.0)).center();
    click(vcx, slot);
    assert!(order(&fixture, vcx).is_empty());
    assert_eq!(active(&fixture, vcx), 0);
    assert_eq!(strip_state(&fixture, vcx), (Vec::new(), None));
    assert_eq!(fixture.closed.borrow().last(), Some(&(b, 0)));
}

#[vespertine::test]
fn dragging_a_chip_reorders_the_panel_tabs_and_the_active_tab_follows(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, 4, true);
    let [a, b, c, d] = [
        fixture.tabs[0],
        fixture.tabs[1],
        fixture.tabs[2],
        fixture.tabs[3],
    ];
    // Active is `d`. Dragging `a` (index 0) to index 2 moves it past `b` and `c`; the active tab
    // keeps its identity and index 3.
    let from = chip(vcx, a);
    let to = chip(vcx, c);
    let none = Modifiers::none();
    vcx.simulate_mouse_move(from, None, none);
    redraw(vcx);
    vcx.simulate_mouse_down(from, MouseButton::Left, none);
    vcx.simulate_mouse_move(from + point(px(6.), px(0.)), MouseButton::Left, none);
    redraw(vcx);
    vcx.simulate_mouse_move(to, MouseButton::Left, none);
    redraw(vcx);
    vcx.simulate_mouse_up(to, MouseButton::Left, none);
    redraw(vcx);
    assert_eq!(order(&fixture, vcx), vec![b, c, a, d]);
    assert_eq!(active(&fixture, vcx), 3);
    assert_eq!(
        strip_state(&fixture, vcx),
        (ids(&[b, c, a, d]), Some(d.0.to_string()))
    );

    // Dragging the active tab itself: the active index follows the move (R20).
    let from = chip(vcx, d);
    let to = chip(vcx, b);
    vcx.simulate_mouse_move(from, None, none);
    redraw(vcx);
    vcx.simulate_mouse_down(from, MouseButton::Left, none);
    vcx.simulate_mouse_move(from - point(px(6.), px(0.)), MouseButton::Left, none);
    redraw(vcx);
    vcx.simulate_mouse_move(to, MouseButton::Left, none);
    redraw(vcx);
    vcx.simulate_mouse_up(to, MouseButton::Left, none);
    redraw(vcx);
    assert_eq!(order(&fixture, vcx), vec![d, b, c, a]);
    assert_eq!(active(&fixture, vcx), 0);
}

#[vespertine::test]
fn host_driven_changes_are_mirrored_into_the_strip(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, 3, true);
    let [a, b, c] = [fixture.tabs[0], fixture.tabs[1], fixture.tabs[2]];
    fixture
        .panel
        .update(vcx, |panel, cx| panel.move_tab(0, 2, cx));
    assert_eq!(strip_state(&fixture, vcx).0, ids(&[b, c, a]));
    fixture
        .panel
        .update(vcx, |panel, cx| panel.select_tab_id(b, cx));
    assert_eq!(strip_state(&fixture, vcx).1, Some(b.0.to_string()));
    fixture.panel.update(vcx, |panel, cx| {
        panel.close_tab(b, cx);
    });
    let (listed, selected) = strip_state(&fixture, vcx);
    assert_eq!(listed, ids(&[c, a]));
    // Closing the active tab at index 0 keeps index 0 (R20): the tab after it.
    assert_eq!(
        selected,
        Some(c.0.to_string()),
        "the panel's survivor is selected"
    );
    assert!(
        fixture.closed.borrow().is_empty(),
        "a close the host asked for is not reported back"
    );
}

#[vespertine::test]
fn the_new_tab_button_asks_the_host_or_appends_a_ready_tab(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, 2, true);
    let add = bounds(vcx, "tab-add".into()).center();
    click(vcx, add);
    assert_eq!(*fixture.new_tab_requests.borrow(), 1);
    assert_eq!(
        order(&fixture, vcx).len(),
        2,
        "the host decides when the session exists"
    );

    let (fixture, vcx) = setup(cx, 2, false);
    let add = bounds(vcx, "tab-add".into()).center();
    click(vcx, add);
    let tabs = order(&fixture, vcx);
    assert_eq!(tabs.len(), 3);
    assert_eq!(active(&fixture, vcx), 2, "a new tab is appended and active");
    assert_eq!(strip_state(&fixture, vcx).1, Some(tabs[2].0.to_string()));
}
