//! CMP-017 (tab strip): selection, cycling, closing, overflow, drag reordering.

use std::{cell::RefCell, rc::Rc};
use vespertine::{
    AppContext, Bounds, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MotionClock, MouseButton, ParentElement,
    Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Styled, TestAppContext, TouchPhase,
    VisualTestContext, Window, div, point, px,
};
use vespertine_components::tabs::{
    CloseDecision, ClosePolicy, NextTab, PreviousTab, TabId, TabSpec, TabStrip, bind_keys, cycle_on,
};

type Log = Rc<RefCell<Vec<String>>>;

struct Root {
    strip: Entity<TabStrip>,
    contents: Vec<(TabId, FocusHandle)>,
    width: f32,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.strip.read(cx).selected_id().cloned();
        let shown = self
            .contents
            .iter()
            .find(|(id, _)| Some(id) == selected.as_ref())
            .map(|(id, focus)| {
                div()
                    .id(format!("content:{id}"))
                    .track_focus(focus)
                    .tab_stop(true)
                    .size(px(50.))
            });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().w(px(self.width)).child(self.strip.clone()))
            .child(cycle_on(div().id("content-region").size(px(60.)), &self.strip).children(shown))
    }
}

struct Fixture {
    strip: Entity<TabStrip>,
    root: Entity<Root>,
    log: Log,
}

fn specs(names: &[&str]) -> Vec<TabSpec> {
    names
        .iter()
        .map(|name| TabSpec::new(*name, name.to_uppercase()))
        .collect()
}

fn id(name: &str) -> TabId {
    TabId::from(name)
}

fn setup<'a>(
    cx: &'a mut TestAppContext,
    names: &[&str],
    selected: Option<&str>,
    width: f32,
    configure: impl FnOnce(TabStrip, &Log) -> TabStrip,
) -> (Fixture, &'a mut VisualTestContext) {
    let log: Log = Rc::default();
    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    let selected = selected.map(TabId::from);
    let log2 = log.clone();
    let (root, vcx) = cx.add_window_view(move |window, cx| {
        let sink = log2.clone();
        let strip = TabStrip::new("main")
            .tabs(specs(&names.iter().map(String::as_str).collect::<Vec<_>>()))
            .selected(selected)
            .on_select({
                let sink = sink.clone();
                move |id, _, _| sink.borrow_mut().push(format!("select:{id}"))
            })
            .on_close({
                let sink = sink.clone();
                move |id, _, _| sink.borrow_mut().push(format!("close:{id}"))
            })
            .on_reorder({
                let sink = sink.clone();
                move |from, to, _, _| sink.borrow_mut().push(format!("reorder:{from}>{to}"))
            })
            .on_context_menu(move |id, _, _, _| sink.borrow_mut().push(format!("menu:{id}")));
        let strip = configure(strip, &log2);
        let strip = cx.new(|_| strip);
        let contents: Vec<(TabId, FocusHandle)> = names
            .iter()
            .map(|name| (TabId::from(name.as_str()), cx.focus_handle()))
            .collect();
        strip.update(cx, |strip, _| {
            for (id, focus) in &contents {
                strip.set_content_focus(id.clone(), focus.clone());
            }
        });
        cx.observe(&strip, |_, _, cx| cx.notify()).detach();
        let _ = window;
        Root {
            strip,
            contents,
            width,
        }
    });
    vcx.update(|window, app| {
        window.activate_window();
        bind_keys(app);
    });
    let strip = root.read_with(vcx, |root, _| root.strip.clone());
    let fixture = Fixture { strip, root, log };
    redraw(vcx);
    redraw(vcx);
    (fixture, vcx)
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn bounds(cx: &mut VisualTestContext, selector: String) -> Bounds<Pixels> {
    cx.debug_bounds(&selector)
        .unwrap_or_else(|| panic!("no element {selector}"))
}

fn tab_center(cx: &mut VisualTestContext, name: &str) -> Point<Pixels> {
    bounds(cx, format!("tab:{name}")).center()
}

fn log_of(fixture: &Fixture) -> Vec<String> {
    fixture.log.borrow().clone()
}

fn order(fixture: &Fixture, cx: &mut VisualTestContext) -> Vec<String> {
    fixture.strip.read_with(cx, |strip, _| {
        strip.specs().iter().map(|tab| tab.id.to_string()).collect()
    })
}

fn selected(fixture: &Fixture, cx: &mut VisualTestContext) -> Option<String> {
    fixture
        .strip
        .read_with(cx, |strip, _| strip.selected_id().map(ToString::to_string))
}

fn close(fixture: &Fixture, cx: &mut VisualTestContext, name: &str) {
    let strip = fixture.strip.clone();
    cx.update(|window, app| {
        strip.update(app, |strip, cx| strip.close_tab(&id(name), window, cx));
    });
    redraw(cx);
}

/// Presses and releases a key, as the platform delivers it to a focused chip.
fn press(cx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).expect("a keystroke");
    cx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke });
    redraw(cx);
}

fn click(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_mouse_move(position, None, Modifiers::default());
    redraw(cx);
    cx.simulate_click(position, Modifiers::default());
    redraw(cx);
}

/// Presses on `from`, drags to `to` in small steps and releases there.
fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    cx.simulate_mouse_move(from, None, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        from + point(px(6.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    redraw(cx);
}

// ---------------------------------------------------------------------------------------------
// R1, R2: identity, click selection, keyboard activation.
// ---------------------------------------------------------------------------------------------

#[vespertine::test]
fn clicking_a_tab_selects_it_and_enter_or_space_activates_the_focused_tab(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("a"), 600.0, |strip, _| strip);
    let center = tab_center(cx, "b");
    click(cx, center);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("b"));
    assert_eq!(log_of(&fixture), ["select:b"]);

    // Tab chips are keyboard tab stops: focus one, Enter and Space activate it.
    cx.update(|window, app| {
        window.blur(app);
        window.focus_next(app);
    });
    redraw(cx);
    let focused = cx.update(|window, app| {
        fixture
            .strip
            .read(app)
            .focused_tab(window)
            .map(ToString::to_string)
    });
    assert_eq!(focused.as_deref(), Some("a"));
    press(cx, "enter");
    assert_eq!(selected(&fixture, cx).as_deref(), Some("a"));
    cx.update(|window, app| window.focus_next(app));
    redraw(cx);
    press(cx, "space");
    assert_eq!(selected(&fixture, cx).as_deref(), Some("b"));
    assert_eq!(log_of(&fixture), ["select:b", "select:a", "select:b"]);
}

#[vespertine::test]
fn a_press_that_starts_a_drag_does_not_select(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("a"), 600.0, |strip, _| strip);
    let from = tab_center(cx, "c");
    let to = tab_center(cx, "b");
    drag(cx, from, to);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("a"));
    assert!(
        !log_of(&fixture)
            .iter()
            .any(|entry| entry.starts_with("select"))
    );
}

// ---------------------------------------------------------------------------------------------
// R3: cycling.
// ---------------------------------------------------------------------------------------------

#[vespertine::test]
fn ctrl_tab_cycles_with_wrap_and_ctrl_shift_tab_goes_back(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("a"), 600.0, |strip, _| strip);
    // Focus the content region so the action reaches the strip through `cycle_on`.
    let content = fixture
        .root
        .read_with(cx, |root, _| root.contents[0].1.clone());
    cx.update(|window, app| window.focus(&content, app));
    redraw(cx);
    cx.simulate_keystrokes("ctrl-tab");
    redraw(cx);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("b"));
    cx.simulate_keystrokes("ctrl-tab ctrl-tab");
    redraw(cx);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("a"));
    cx.simulate_keystrokes("ctrl-shift-tab");
    redraw(cx);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("c"));
    assert_eq!(
        log_of(&fixture),
        ["select:b", "select:c", "select:a", "select:c"]
    );
}

#[vespertine::test]
fn cycling_moves_focus_into_the_new_content_only_when_focus_was_in_the_content(
    cx: &mut TestAppContext,
) {
    let (fixture, cx) = setup(cx, &["a", "b"], Some("a"), 600.0, |strip, _| strip);
    let (a, b) = fixture.root.read_with(cx, |root, _| {
        (root.contents[0].1.clone(), root.contents[1].1.clone())
    });
    cx.update(|window, app| window.focus(&a, app));
    redraw(cx);
    cx.simulate_keystrokes("ctrl-tab");
    redraw(cx);
    assert!(cx.update(|window, _| b.is_focused(window)));

    // With focus outside the content region (nothing focused), cycling leaves focus alone.
    cx.update(|window, app| window.blur(app));
    redraw(cx);
    let strip = fixture.strip.clone();
    cx.update(|window, app| strip.update(app, |strip, cx| strip.cycle(true, window, cx)));
    redraw(cx);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("a"));
    assert!(!cx.update(|window, _| a.is_focused(window)));
}

#[vespertine::test]
fn cycling_with_nothing_selected_picks_the_ends_and_one_tab_changes_nothing(
    cx: &mut TestAppContext,
) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], None, 600.0, |strip, _| strip);
    let strip = fixture.strip.clone();
    cx.update(|window, app| strip.update(app, |strip, cx| strip.cycle(true, window, cx)));
    assert_eq!(selected(&fixture, cx).as_deref(), Some("a"));
    cx.update(|_, app| strip.update(app, |strip, cx| strip.clear_selection(cx)));
    cx.update(|window, app| strip.update(app, |strip, cx| strip.cycle(false, window, cx)));
    assert_eq!(selected(&fixture, cx).as_deref(), Some("c"));

    let (single, cx) = setup(cx, &["only"], Some("only"), 600.0, |strip, _| strip);
    let changed = cx.update(|window, app| {
        single
            .strip
            .update(app, |strip, cx| strip.cycle(true, window, cx))
    });
    assert!(!changed);
    assert!(log_of(&single).is_empty());
}

#[vespertine::test]
fn cycling_is_suppressed_while_an_overlay_owns_the_keyboard(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b"], Some("a"), 600.0, |strip, _| strip);
    let content = fixture
        .root
        .read_with(cx, |root, _| root.contents[0].1.clone());
    cx.update(|window, app| window.focus(&content, app));
    fixture
        .strip
        .update(cx, |strip, _| strip.set_overlay_active(true));
    cx.simulate_keystrokes("ctrl-tab");
    redraw(cx);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("a"));
    fixture
        .strip
        .update(cx, |strip, _| strip.set_overlay_active(false));
    cx.simulate_keystrokes("ctrl-tab");
    redraw(cx);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("b"));
    let _ = (NextTab, PreviousTab);
}

// ---------------------------------------------------------------------------------------------
// R4, R5: closing and the surviving selection.
// ---------------------------------------------------------------------------------------------

#[vespertine::test]
fn nearest_index_survivor_table_and_the_last_tab_leaves_no_selection(cx: &mut TestAppContext) {
    // Close left of selected (selected 2, closed 0, 3 remain) -> 1.
    let (fixture, cx) = setup(cx, &["a", "b", "c", "d"], Some("c"), 600.0, |strip, _| {
        strip
    });
    close(&fixture, cx, "a");
    assert_eq!(selected(&fixture, cx).as_deref(), Some("c"));
    assert_eq!(order(&fixture, cx), ["b", "c", "d"]);
    // Closing an inactive tab never changes the selection.
    close(&fixture, cx, "d");
    assert_eq!(selected(&fixture, cx).as_deref(), Some("c"));
    // Close the selected tab at the tail (selected 1, closed 1, 1 remains) -> the first.
    close(&fixture, cx, "c");
    assert_eq!(selected(&fixture, cx).as_deref(), Some("b"));
    // The last tab leaves no selection.
    close(&fixture, cx, "b");
    assert_eq!(selected(&fixture, cx), None);
    assert!(order(&fixture, cx).is_empty());
    assert_eq!(
        log_of(&fixture),
        ["close:a", "close:d", "close:c", "select:b", "close:b"]
    );

    // Close the selected tab mid-list (selected 1, closed 1, 2 remain) -> 1: the next slides in.
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("b"), 600.0, |strip, _| strip);
    close(&fixture, cx, "b");
    assert_eq!(selected(&fixture, cx).as_deref(), Some("c"));
}

#[vespertine::test]
fn most_recently_visited_policy_selects_the_newest_live_history_entry(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c", "d"], Some("a"), 600.0, |strip, _| {
        strip.close_policy(ClosePolicy::MostRecentlyVisited)
    });
    // Visits a, then c, d, b: the history is [a c d b].
    let strip = fixture.strip.clone();
    for name in ["c", "d", "b"] {
        cx.update(|_, app| strip.update(app, |strip, cx| strip.select(&id(name), cx)));
    }
    // Reordering and the empty state are not visits.
    let history = |cx: &mut VisualTestContext| {
        strip.read_with(cx, |strip, _| {
            strip
                .visit_history()
                .history("main")
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(history(cx), ["a", "c", "d", "b"]);
    let from = tab_center(cx, "d");
    let to = tab_center(cx, "a");
    drag(cx, from, to);
    assert_eq!(order(&fixture, cx), ["d", "a", "b", "c"]);
    assert_eq!(history(cx), ["a", "c", "d", "b"]);
    // Closing b (selected) falls back to d, the most recent live entry; the strip order
    // [d a c] plays no role.
    close(&fixture, cx, "b");
    assert_eq!(selected(&fixture, cx).as_deref(), Some("d"));
    close(&fixture, cx, "d");
    assert_eq!(selected(&fixture, cx).as_deref(), Some("c"));
    // Only a is left in the history after c closes: the first tab is chosen.
    close(&fixture, cx, "c");
    assert_eq!(selected(&fixture, cx).as_deref(), Some("a"));
}

#[vespertine::test]
fn closing_the_selected_tab_moves_focus_from_its_content_to_the_survivor(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("b"), 600.0, |strip, _| strip);
    let (b, c) = fixture.root.read_with(cx, |root, _| {
        (root.contents[1].1.clone(), root.contents[2].1.clone())
    });
    cx.update(|window, app| window.focus(&b, app));
    redraw(cx);
    close(&fixture, cx, "b");
    redraw(cx);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("c"));
    assert!(cx.update(|window, _| c.is_focused(window)));
}

#[vespertine::test]
fn closing_an_inactive_tab_leaves_focus_alone(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("b"), 600.0, |strip, _| strip);
    let b = fixture
        .root
        .read_with(cx, |root, _| root.contents[1].1.clone());
    cx.update(|window, app| window.focus(&b, app));
    redraw(cx);
    close(&fixture, cx, "c");
    redraw(cx);
    assert!(cx.update(|window, _| b.is_focused(window)));
    assert_eq!(selected(&fixture, cx).as_deref(), Some("b"));
}

#[vespertine::test]
fn a_pending_veto_keeps_the_tab_selects_it_and_removes_nothing(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("a"), 600.0, |strip, _| {
        strip.prepare_close(|id, _, _| {
            if id.as_str() == "c" {
                CloseDecision::Pending
            } else if id.as_str() == "b" {
                CloseDecision::Blocked
            } else {
                CloseDecision::Allow
            }
        })
    });
    close(&fixture, cx, "c");
    assert_eq!(order(&fixture, cx), ["a", "b", "c"]);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("c"));
    assert_eq!(
        fixture
            .strip
            .read_with(cx, |strip, _| strip.pending_close().cloned()),
        Some(id("c"))
    );
    assert_eq!(log_of(&fixture), ["select:c"]);
    close(&fixture, cx, "b");
    assert_eq!(order(&fixture, cx), ["a", "b", "c"]);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("b"));
    // The owner resolves the pending close: allowing removes the tab.
    let strip = fixture.strip.clone();
    cx.update(|window, app| {
        strip.update(app, |strip, cx| {
            strip.resolve_close(&id("b"), true, window, cx)
        })
    });
    redraw(cx);
    assert_eq!(order(&fixture, cx), ["a", "c"]);
    assert!(log_of(&fixture).contains(&"close:b".to_owned()));
}

#[vespertine::test]
fn middle_click_closes_the_tab_and_right_click_opens_the_context_menu(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("a"), 600.0, |strip, _| strip);
    let center = tab_center(cx, "b");
    cx.simulate_mouse_move(center, None, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_down(center, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(center, MouseButton::Right, Modifiers::default());
    redraw(cx);
    assert_eq!(log_of(&fixture), ["menu:b"]);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("a"));
    cx.simulate_mouse_down(center, MouseButton::Middle, Modifiers::default());
    cx.simulate_mouse_up(center, MouseButton::Middle, Modifiers::default());
    redraw(cx);
    assert_eq!(order(&fixture, cx), ["a", "c"]);
    assert_eq!(log_of(&fixture), ["menu:b", "close:b"]);
}

#[vespertine::test]
fn the_close_slot_is_18_px_keeps_the_title_width_and_never_starts_a_drag(cx: &mut TestAppContext) {
    let log: Log = Rc::default();
    let _ = log;
    let (fixture, cx) = setup(cx, &["a", "b"], Some("a"), 600.0, |strip, _| strip);
    fixture.strip.update(cx, |strip, cx| {
        let mut tabs = specs(&["a", "b"]);
        tabs[0].dirty = true;
        strip.set_tabs(tabs, cx);
    });
    redraw(cx);
    let slot = bounds(cx, "tab-close:a".into());
    assert_eq!((slot.size.width, slot.size.height), (px(18.), px(18.)));
    let chip = bounds(cx, "tab:a".into());
    // 4 px inset from the chip's right edge.
    assert_eq!(chip.right() - slot.right(), px(4.));
    let title_before = bounds(cx, "tab-title:a".into());
    cx.simulate_mouse_move(chip.center(), None, Modifiers::default());
    redraw(cx);
    let title_hovered = bounds(cx, "tab-title:a".into());
    assert_eq!(title_before.size, title_hovered.size);
    assert_eq!(bounds(cx, "tab-close:a".into()), slot);

    // Pressing the slot and moving never starts a drag, and does not select either tab.
    let center = slot.center();
    cx.simulate_mouse_move(center, None, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        center + point(px(30.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    assert!(!cx.update(|_, app| app.has_active_drag()));
    assert!(!fixture.strip.read_with(cx, |strip, _| strip.is_dragging()));
    cx.simulate_mouse_up(
        center + point(px(30.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    assert_eq!(order(&fixture, cx), ["a", "b"]);
}

#[vespertine::test]
fn clicking_the_close_slot_closes_the_tab(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b"], Some("a"), 600.0, |strip, _| strip);
    let slot = bounds(cx, "tab-close:b".into()).center();
    click(cx, slot);
    assert_eq!(order(&fixture, cx), ["a"]);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("a"));
    assert_eq!(log_of(&fixture), ["close:b"]);
}

// ---------------------------------------------------------------------------------------------
// R9, R10: overflow.
// ---------------------------------------------------------------------------------------------

fn many() -> Vec<&'static str> {
    vec!["t0", "t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8", "t9"]
}

fn scroll_to(fixture: &Fixture, cx: &mut VisualTestContext, x: f32) {
    let scroll = fixture
        .strip
        .read_with(cx, |strip, _| strip.scroll().clone());
    scroll.set_offset(point(px(-x), px(0.)));
    redraw(cx);
    redraw(cx);
}

#[vespertine::test]
fn edge_flags_follow_the_scroll_with_a_one_pixel_tolerance(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &many(), Some("t0"), 300.0, |strip, _| strip);
    let flags = |cx: &mut VisualTestContext| fixture.strip.read_with(cx, |s, _| s.edge_hidden());
    // 10 tabs: 10 * 112 + 9 * 4 = 1156 px of content in 300 px: max scroll 856.
    assert_eq!(flags(cx), (false, true));
    scroll_to(&fixture, cx, 1.0);
    assert_eq!(flags(cx), (false, true));
    scroll_to(&fixture, cx, 1.5);
    assert_eq!(flags(cx), (true, true));
    scroll_to(&fixture, cx, 400.0);
    assert_eq!(flags(cx), (true, true));
    scroll_to(&fixture, cx, 854.5);
    assert_eq!(flags(cx), (true, true));
    scroll_to(&fixture, cx, 855.5);
    assert_eq!(flags(cx), (true, false));
    scroll_to(&fixture, cx, 856.0);
    assert_eq!(flags(cx), (true, false));
}

#[vespertine::test]
fn the_wheel_over_a_chip_scrolls_the_strip_horizontally(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &many(), Some("t0"), 300.0, |strip, _| strip);
    let center = tab_center(cx, "t1");
    cx.simulate_mouse_move(center, None, Modifiers::default());
    redraw(cx);
    cx.simulate_event(ScrollWheelEvent {
        position: center,
        delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    redraw(cx);
    let offset = fixture
        .strip
        .read_with(cx, |strip, _| strip.scroll().offset().x);
    assert!(offset < px(-1.), "the strip scrolled left: {offset:?}");
}

#[vespertine::test]
fn cycling_to_a_tab_scrolled_out_of_view_reveals_it_with_minimal_scroll(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &many(), Some("t0"), 300.0, |strip, _| strip);
    scroll_to(&fixture, cx, 600.0);
    let hidden =
        |cx: &mut VisualTestContext| fixture.strip.read_with(cx, |s, _| s.selected_hidden());
    assert!(hidden(cx), "t0 is scrolled out of view before the reveal");
    let strip = fixture.strip.clone();
    cx.update(|window, app| strip.update(app, |strip, cx| strip.cycle(true, window, cx)));
    redraw(cx);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("t1"));
    // The minimal scroll puts t1 (content x 116..228) at the left edge of the viewport.
    let offset = fixture
        .strip
        .read_with(cx, |strip, _| strip.scroll().offset().x);
    assert_eq!(offset, px(-116.));
    assert!(!hidden(cx));
    // Cycling backwards past the start wraps to the last tab and reveals it at the right edge.
    cx.update(|window, app| strip.update(app, |strip, cx| strip.cycle(false, window, cx)));
    cx.update(|window, app| strip.update(app, |strip, cx| strip.cycle(false, window, cx)));
    redraw(cx);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("t9"));
    let offset = fixture
        .strip
        .read_with(cx, |strip, _| strip.scroll().offset().x);
    assert_eq!(offset, px(-856.));
    assert!(!hidden(cx));
}

#[vespertine::test]
fn scrolling_by_hand_never_reveals_an_unchanged_selection(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &many(), Some("t0"), 300.0, |strip, _| strip);
    scroll_to(&fixture, cx, 500.0);
    redraw(cx);
    let offset = fixture
        .strip
        .read_with(cx, |strip, _| strip.scroll().offset().x);
    assert_eq!(offset, px(-500.));
    // scroll_to_tab is the explicit way to reveal.
    let strip = fixture.strip.clone();
    cx.update(|_, app| strip.update(app, |strip, cx| strip.scroll_to_tab(&id("t9"), cx)));
    redraw(cx);
    assert_eq!(
        fixture.strip.read_with(cx, |s, _| s.scroll().offset().x),
        px(-856.)
    );
}

// ---------------------------------------------------------------------------------------------
// R11, R12: drag reordering.
// ---------------------------------------------------------------------------------------------

#[vespertine::test]
fn dragging_a_tab_over_the_third_reorders_and_keeps_the_selection(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c", "d"], Some("b"), 600.0, |strip, _| {
        strip
    });
    let from = tab_center(cx, "a");
    let to = tab_center(cx, "c");
    drag(cx, from, to);
    assert_eq!(order(&fixture, cx), ["b", "c", "a", "d"]);
    assert_eq!(selected(&fixture, cx).as_deref(), Some("b"));
    assert_eq!(log_of(&fixture), ["reorder:0>2"]);
    // Release: the drag state is gone.
    assert!(!fixture.strip.read_with(cx, |s, _| s.is_dragging()));
}

#[vespertine::test]
fn a_drop_on_the_source_position_changes_nothing_and_emits_no_callback(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("a"), 600.0, |strip, _| strip);
    let from = tab_center(cx, "b");
    drag(cx, from, from + point(px(10.), px(0.)));
    assert_eq!(order(&fixture, cx), ["a", "b", "c"]);
    assert!(log_of(&fixture).is_empty());
    // A drop beyond the strip's last tab clamps to the last index.
    let from = tab_center(cx, "a");
    let beyond = point(px(500.), from.y);
    drag(cx, from, beyond);
    assert_eq!(order(&fixture, cx), ["b", "c", "a"]);
    assert_eq!(log_of(&fixture), ["reorder:0>2"]);
}

#[vespertine::test]
fn a_drag_from_the_body_slides_siblings_and_retargets_mid_flight(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c", "d"], Some("a"), 600.0, |strip, _| {
        strip
    });
    cx.update(|_, app| {
        MotionClock::set(app, 1000.0);
    });
    let from = tab_center(cx, "a");
    cx.simulate_mouse_move(from, None, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        from + point(px(6.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    assert!(fixture.strip.read_with(cx, |s, _| s.is_dragging()));
    // Over the third tab: tabs 1 and 2 slide one slot toward the vacated place.
    let over = tab_center(cx, "c");
    cx.simulate_mouse_move(over, MouseButton::Left, Modifiers::default());
    redraw(cx);
    let (target_over, epoch) = fixture
        .strip
        .read_with(cx, |s, _| (s.drag_over(), s.drag_epoch()));
    assert_eq!(target_over, Some(2));
    let at = |cx: &mut VisualTestContext, ix: usize, now: f64| {
        fixture.strip.read_with(cx, |s, _| s.slide_px(ix, now))
    };
    assert_eq!(at(cx, 1, 1000.0), 0.0);
    assert_eq!(at(cx, 1, 1150.0), -116.0);
    assert_eq!(at(cx, 2, 1150.0), -116.0);
    assert_eq!(at(cx, 3, 1150.0), 0.0);
    // Half way through, reverse: the animation restarts from the displaced position.
    cx.update(|_, app| {
        MotionClock::set(app, 1075.0);
    });
    let displaced = at(cx, 1, 1075.0);
    assert!(displaced < -58.0 && displaced > -116.0);
    let back = tab_center(cx, "a");
    cx.simulate_mouse_move(back, MouseButton::Left, Modifiers::default());
    redraw(cx);
    let (over_now, epoch_now) = fixture
        .strip
        .read_with(cx, |s, _| (s.drag_over(), s.drag_epoch()));
    assert_eq!(over_now, Some(0));
    assert!(epoch_now > epoch);
    assert!(
        (at(cx, 1, 1075.0) - displaced).abs() < 0.01,
        "no jump on retarget"
    );
    assert_eq!(at(cx, 1, 1225.0), 0.0);
    cx.simulate_mouse_up(back, MouseButton::Left, Modifiers::default());
    redraw(cx);
}

#[vespertine::test]
fn reduced_motion_snaps_the_slide_offsets(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("a"), 600.0, |strip, _| strip);
    cx.update(|_, app| {
        MotionClock::set(app, 500.0);
        app.set_reduce_motion(true);
    });
    let from = tab_center(cx, "a");
    cx.simulate_mouse_move(from, None, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        from + point(px(6.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    let over = tab_center(cx, "c");
    cx.simulate_mouse_move(over, MouseButton::Left, Modifiers::default());
    redraw(cx);
    let value = fixture.strip.read_with(cx, |s, _| s.slide_px(1, 500.0));
    assert_eq!(value, -116.0);
    cx.simulate_mouse_up(over, MouseButton::Left, Modifiers::default());
    redraw(cx);
}

#[vespertine::test]
fn a_payload_of_another_strip_is_rejected(cx: &mut TestAppContext) {
    use vespertine::{AppContext as _, ParentElement as _};
    struct Two {
        first: Entity<TabStrip>,
        second: Entity<TabStrip>,
    }
    impl Render for Two {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().w(px(600.)).child(self.first.clone()))
                .child(div().w(px(600.)).mt(px(40.)).child(self.second.clone()))
        }
    }
    let log: Log = Rc::default();
    let sink = log.clone();
    let (two, cx) = cx.add_window_view(move |_, cx| {
        let first = cx.new(|_| {
            TabStrip::new("one")
                .tabs(specs(&["a", "b", "c"]))
                .selected(id("a"))
        });
        let second = cx.new(|_| {
            TabStrip::new("two")
                .tabs(specs(&["x", "y", "z"]))
                .selected(id("x"))
                .on_reorder(move |from, to, _, _| {
                    sink.borrow_mut().push(format!("two:{from}>{to}"))
                })
        });
        Two { first, second }
    });
    redraw(cx);
    redraw(cx);
    let from = bounds(cx, "tab:a".into()).center();
    let onto = bounds(cx, "tab:z".into()).center();
    drag(cx, from, onto);
    let (first, second) = two.read_with(cx, |two, _| (two.first.clone(), two.second.clone()));
    let names = |strip: &Entity<TabStrip>, cx: &mut VisualTestContext| {
        strip.read_with(cx, |strip, _| {
            strip
                .specs()
                .iter()
                .map(|tab| tab.id.to_string())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(names(&second, cx), ["x", "y", "z"]);
    assert_eq!(names(&first, cx), ["a", "b", "c"]);
    assert!(log.borrow().is_empty());
    // Both strips are back to rest after the next frame.
    assert!(!first.read_with(cx, |s, _| s.is_dragging()));
    assert!(!second.read_with(cx, |s, _| s.is_dragging()));
}

#[vespertine::test]
fn releasing_outside_any_strip_clears_the_drag_state_by_the_next_frame(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c"], Some("a"), 600.0, |strip, _| strip);
    let from = tab_center(cx, "a");
    cx.simulate_mouse_move(from, None, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        from + point(px(6.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    assert!(fixture.strip.read_with(cx, |s, _| s.is_dragging()));
    let outside = point(px(300.), px(400.));
    cx.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    redraw(cx);
    // Outside the strip the pointer hovers the origin: nothing slides.
    assert_eq!(fixture.strip.read_with(cx, |s, _| s.drag_over()), Some(0));
    cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    redraw(cx);
    assert!(!fixture.strip.read_with(cx, |s, _| s.is_dragging()));
    assert_eq!(order(&fixture, cx), ["a", "b", "c"]);
    assert!(log_of(&fixture).is_empty());
}

#[vespertine::test]
fn a_drop_on_a_chip_that_blocks_the_pointer_still_commits(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, &["a", "b", "c", "d"], Some("a"), 600.0, |strip, _| {
        strip
    });
    let from = tab_center(cx, "d");
    // The drop lands in the middle of chip b, not in a gap.
    let to = tab_center(cx, "b");
    drag(cx, from, to);
    assert_eq!(order(&fixture, cx), ["a", "d", "b", "c"]);
    assert_eq!(log_of(&fixture), ["reorder:3>1"]);
}

#[vespertine::test]
fn reordering_keeps_content_focus_and_scroll_drop_math_uses_the_scroll_offset(
    cx: &mut TestAppContext,
) {
    let (fixture, cx) = setup(cx, &many(), Some("t0"), 300.0, |strip, _| strip);
    let first = fixture
        .root
        .read_with(cx, |root, _| root.contents[0].1.clone());
    cx.update(|window, app| window.focus(&first, app));
    redraw(cx);
    scroll_to(&fixture, cx, 232.0);
    // With the strip scrolled by two slots, the chip under x is two indices further.
    let from = tab_center(cx, "t2");
    let target = tab_center(cx, "t3");
    drag(cx, from, target);
    assert_eq!(order(&fixture, cx)[..5], ["t0", "t1", "t3", "t2", "t4"]);
    assert!(cx.update(|window, _| first.is_focused(window)));
}

// ---------------------------------------------------------------------------------------------
// R15: accessibility.
// ---------------------------------------------------------------------------------------------

#[vespertine::test]
fn chips_expose_tab_semantics_with_selected_state_unsaved_suffix_and_close_label(
    cx: &mut TestAppContext,
) {
    let (fixture, cx) = setup(cx, &["a", "b"], Some("b"), 600.0, |strip, _| strip);
    fixture.strip.update(cx, |strip, cx| {
        let mut tabs = specs(&["a", "b"]);
        tabs[0].dirty = true;
        tabs[1].accessible_name = Some("Beta document".into());
        strip.set_tabs(tabs, cx);
    });
    cx.update(|window, _| window.set_a11y_active_for_tests(true));
    redraw(cx);
    redraw(cx);
    let tree = cx
        .update(|window, _| window.debug_a11y_tree_json())
        .expect("an accessibility tree");
    let tree: serde_json::Value = serde_json::from_str(&tree).expect("json");
    let nodes = tree["nodes"].as_object().expect("nodes");
    let with_role = |role: &str| -> Vec<&serde_json::Value> {
        nodes
            .values()
            .filter(|node| node["aria"]["role"] == role)
            .map(|node| &node["aria"])
            .collect()
    };
    let tabs = with_role("Tab");
    assert_eq!(tabs.len(), 2);
    let label_of = |label: &str| tabs.iter().find(|tab| tab["label"] == label).copied();
    // A dirty tab's accessible name gains ", unsaved changes"; a longer detail name replaces the
    // title; only the selected tab reports selected.
    let dirty = label_of("A, unsaved changes").expect("the dirty tab");
    assert!(dirty["selected"].is_null() || dirty["selected"] == false);
    let named = label_of("Beta document").expect("the tab with a longer name");
    assert_eq!(named["selected"], true);
    // The close slots are labelled buttons; the strip is a tab list.
    let closers = with_role("Button");
    assert_eq!(
        closers.iter().filter(|b| b["label"] == "Close tab").count(),
        2
    );
    assert_eq!(with_role("TabList").len(), 1);
}
