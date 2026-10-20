//! CMP-021: the terminal surface in the offscreen production-path test host (input routing,
//! coalescing, pointer selection, edge scrolling, wheel, tabs, dock and painted primitives).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use vespertine::{
    AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Hsla, InteractiveElement,
    IntoElement, KeyBinding, Modifiers, MouseButton, MouseDownEvent, ParentElement, Render,
    ScrollDelta, ScrollWheelEvent, Styled, TouchPhase, Window, actions, canvas, div, point, px,
};
use vespertine_components::tabs::model::TAB_HEIGHT;
use vespertine_components::terminal::{
    BottomDock, DockGeometry, GridGeometry, TabId, TerminalPanel, TerminalTheme, exit_status_line,
    row_segments, terminal_font, terminal_font_features,
};
use vespertine_test_host::{DEFAULT_SCALE_FACTOR, HostOptions, TestHost};

actions!(terminal_tests, [AppAction, TextAction]);

type Inputs = Rc<RefCell<Vec<(TabId, Vec<u8>)>>>;
type Resizes = Rc<RefCell<Vec<(TabId, usize, usize)>>>;

struct Root {
    panel: Entity<TerminalPanel>,
    other_focus: FocusHandle,
    app_actions: Rc<Cell<usize>>,
    text_actions: Rc<Cell<usize>>,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let app_actions = self.app_actions.clone();
        let text_actions = self.text_actions.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_action(move |_: &AppAction, _, _| app_actions.set(app_actions.get() + 1))
            .on_action(move |_: &TextAction, _, _| text_actions.set(text_actions.get() + 1))
            .child(
                div()
                    .id("elsewhere")
                    .track_focus(&self.other_focus)
                    .w_full()
                    .h(px(20.))
                    .flex_none(),
            )
            .child(div().flex_1().child(self.panel.clone()))
    }
}

struct Harness {
    host: TestHost,
    panel: Entity<TerminalPanel>,
    tab: TabId,
    inputs: Inputs,
    resizes: Resizes,
    app_actions: Rc<Cell<usize>>,
    text_actions: Rc<Cell<usize>>,
    other_focus: FocusHandle,
}

fn harness_with(options: HostOptions) -> Option<Harness> {
    if !vespertine_test_host::is_available() {
        eprintln!("skipping: GPU test backend unavailable");
        return None;
    }
    let inputs: Inputs = Rc::default();
    let resizes: Resizes = Rc::default();
    let app_actions = Rc::new(Cell::new(0));
    let text_actions = Rc::new(Cell::new(0));
    let slot: Rc<RefCell<Option<(Entity<TerminalPanel>, TabId, FocusHandle)>>> = Rc::default();
    let host = TestHost::with_view(options, {
        let inputs = inputs.clone();
        let resizes = resizes.clone();
        let slot = slot.clone();
        let app_actions = app_actions.clone();
        let text_actions = text_actions.clone();
        move |_window, cx| {
            cx.bind_keys([
                KeyBinding::new("cmd-k", AppAction, None),
                KeyBinding::new("backspace", TextAction, Some("TextInput")),
            ]);
            let panel = cx.new(|cx| {
                let mut panel = TerminalPanel::new(cx);
                panel.on_input(move |tab, bytes, _| {
                    inputs.borrow_mut().push((tab, bytes.to_vec()));
                });
                panel.on_resize(move |tab, cols, rows, _| {
                    resizes.borrow_mut().push((tab, cols, rows));
                });
                panel
            });
            let tab = panel.update(cx, |panel, cx| {
                panel.set_theme(Some(test_theme()), cx);
                let tab = panel.open_tab(cx);
                panel.request_focus(cx);
                tab
            });
            let other_focus = cx.focus_handle();
            *slot.borrow_mut() = Some((panel.clone(), tab, other_focus.clone()));
            cx.new(|_| Root {
                panel,
                other_focus,
                app_actions,
                text_actions,
            })
        }
    })
    .expect("host");
    let (panel, tab, other_focus) = slot.borrow_mut().take().expect("panel built");
    let mut harness = Harness {
        host,
        panel,
        tab,
        inputs,
        resizes,
        app_actions,
        text_actions,
        other_focus,
    };
    harness.host.flush();
    harness.host.flush();
    Some(harness)
}

fn harness(width: f64, height: f64) -> Option<Harness> {
    harness_with(HostOptions::with_size(width, height))
}

impl Harness {
    fn feed(&mut self, text: &str) {
        let (panel, tab) = (self.panel.clone(), self.tab);
        self.host
            .app()
            .update(|cx| panel.update(cx, |panel, cx| panel.feed(tab, text.as_bytes(), cx)));
        self.host.flush();
    }

    /// Presents the last drawn frame so the scene observer sees it (frames drawn by the
    /// effect loop are not presented until asked).
    fn present(&mut self) {
        self.host.flush();
        let window = self.host.window();
        let presented = self
            .host
            .app()
            .update_window(window, |_, window, _| window.present_cached_frame())
            .expect("window");
        assert!(presented, "the cached frame can be presented");
    }

    fn geometry(&mut self) -> GridGeometry {
        let panel = self.panel.clone();
        self.host
            .app()
            .update(|cx| panel.read(cx).geometry())
            .expect("measured")
    }

    fn read<R>(&mut self, f: impl FnOnce(&TerminalPanel) -> R) -> R {
        let panel = self.panel.clone();
        self.host.app().update(|cx| f(panel.read(cx)))
    }

    fn update<R>(
        &mut self,
        f: impl FnOnce(&mut TerminalPanel, &mut Window, &mut Context<TerminalPanel>) -> R,
    ) -> R {
        let panel = self.panel.clone();
        let window = self.host.window();
        let result = self
            .host
            .app()
            .update_window(window, |_, window, cx| {
                panel.update(cx, |panel, cx| f(panel, window, cx))
            })
            .expect("window");
        self.host.flush();
        result
    }

    /// The window position of row `row` and column `col` at `fraction` of the cell width.
    fn cell_point(&mut self, row: usize, col: usize, fraction: f32) -> (f64, f64) {
        let geometry = self.geometry();
        (
            f64::from(f32::from(geometry.origin.x) + geometry.cell_width * (col as f32 + fraction)),
            f64::from(f32::from(geometry.origin.y) + geometry.line_height * (row as f32 + 0.5)),
        )
    }

    fn text(&mut self, row: usize) -> String {
        self.read(|panel| panel.active_emulator().expect("tab").row_text(row))
    }

    fn selection(&mut self) -> Option<String> {
        self.read(|panel| panel.active_emulator().expect("tab").selection_text())
    }

    fn offset(&mut self) -> usize {
        self.read(|panel| panel.active_emulator().expect("tab").display_offset())
    }

    fn advance(&mut self, ms: f64) {
        self.host.advance_executor_time(ms).expect("clock");
    }

    fn input_log(&self) -> Vec<Vec<u8>> {
        self.inputs
            .borrow()
            .iter()
            .map(|(_, bytes)| bytes.clone())
            .collect()
    }

    fn press(&mut self, x: f64, y: f64, modifiers: &str) {
        self.host.mouse_down(x, y, 0, modifiers).expect("down");
        self.host.paint_frame();
    }

    fn drag_to(&mut self, x: f64, y: f64) {
        self.host.mouse_move(x, y, Some(0), "").expect("move");
        self.host.paint_frame();
    }

    fn release(&mut self, x: f64, y: f64) {
        self.host.mouse_up(x, y, 0, "").expect("up");
        self.host.paint_frame();
    }

    /// A press with an explicit click count (the host dispatches single clicks only).
    fn click_count(&mut self, x: f64, y: f64, count: usize) {
        let event = MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x as f32), px(y as f32)),
            modifiers: Modifiers::none(),
            click_count: count,
            first_mouse: false,
        };
        self.update(|panel, window, cx| panel.mouse_down(&event, window, cx));
        self.host.paint_frame();
    }
}

/// The neutral dark terminal colors with a fixed-width family that exists on the test machine.
fn test_theme() -> TerminalTheme {
    let mut theme = TerminalTheme::from_variant(&vespertine_theme::builtin::neutral_variant(
        vespertine_theme::theme::Appearance::Dark,
    ));
    theme.font_family = "Liberation Mono".into();
    theme
}

fn lines(count: usize) -> String {
    (1..=count).map(|n| format!("line{n}\r\n")).collect()
}

// --- A7: coalescing in a rendered view ------------------------------------------------------

#[test]
fn a_burst_of_keystrokes_is_flushed_as_one_write_12_ms_after_the_first_key() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    h.host.simulate_keystrokes("a b c", None).expect("keys");
    assert!(
        h.input_log().is_empty(),
        "nothing is written inside the window"
    );
    h.advance(11.0);
    assert!(h.input_log().is_empty());
    h.advance(2.0);
    assert_eq!(h.input_log(), vec![b"abc".to_vec()]);
    h.host.simulate_keystrokes("d", None).expect("keys");
    h.advance(13.0);
    assert_eq!(h.input_log(), vec![b"abc".to_vec(), b"d".to_vec()]);
}

#[test]
fn input_waits_for_the_session_and_is_discarded_after_exit() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    let pending = h.update(|panel, _, cx| panel.open_tab_pending(cx));
    h.update(|panel, _, cx| panel.send_input(pending, b"queued", cx));
    h.advance(12.0);
    h.advance(12.0);
    h.advance(12.0);
    assert!(
        h.input_log().is_empty(),
        "no session yet: the buffer is kept"
    );
    assert!(h.read(|panel| panel.pending_input(pending)));
    h.update(|panel, _, _| panel.set_session_ready(pending, true));
    h.advance(12.0);
    assert_eq!(h.input_log(), vec![b"queued".to_vec()]);

    h.update(|panel, _, cx| panel.send_input(pending, b"late", cx));
    h.update(|panel, _, cx| panel.mark_exited(pending, 3, cx));
    h.advance(30.0);
    assert_eq!(
        h.input_log(),
        vec![b"queued".to_vec()],
        "exited tabs drop input"
    );
    assert_eq!(h.read(|panel| panel.exit_status(pending)), Some(3));
    h.update(|panel, _, cx| panel.send_input(pending, b"more", cx));
    h.advance(30.0);
    assert_eq!(h.input_log().len(), 1);
}

#[test]
fn query_responses_travel_through_the_input_buffer() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    h.feed("\x1b[2;3H\x1b[6n");
    h.advance(13.0);
    assert_eq!(h.input_log(), vec![b"\x1b[2;3R".to_vec()]);
}

// --- Keyboard, clipboard, paste -------------------------------------------------------------

#[test]
fn keys_encode_copy_and_paste_follow_the_clipboard_shortcuts() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    // Control+c is never intercepted.
    h.host
        .simulate_keystrokes("ctrl-c enter up", None)
        .expect("keys");
    h.advance(13.0);
    assert_eq!(h.input_log(), vec![b"\x03\r\x1b[A".to_vec()]);
    // With nothing selected control+shift+c falls through as an interrupt-style key.
    h.host
        .simulate_keystrokes("ctrl-shift-c", None)
        .expect("keys");
    h.advance(13.0);
    assert_eq!(h.input_log().last().cloned(), Some(vec![0x03]));
    assert!(h.host.app().update(|cx| cx.read_from_clipboard()).is_none());

    // Select "hello" and copy it.
    h.feed("hello world");
    let (x0, y) = h.cell_point(0, 0, 0.1);
    let (x1, _) = h.cell_point(0, 4, 0.9);
    h.press(x0, y, "");
    h.drag_to(x1, y);
    h.release(x1, y);
    assert_eq!(h.selection().as_deref(), Some("hello"));
    let before = h.input_log().len();
    h.host
        .simulate_keystrokes("ctrl-shift-c", None)
        .expect("keys");
    h.advance(13.0);
    assert_eq!(h.input_log().len(), before, "a consumed copy sends nothing");
    let copied = h
        .host
        .app()
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("hello"));

    // Paste: bracketed mode wraps and strips the end marker.
    h.feed("\x1b[?2004h");
    h.host.app().update(|cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("a\x1b[201~rm -rf".to_string()))
    });
    h.host
        .simulate_keystrokes("ctrl-shift-v", None)
        .expect("keys");
    h.advance(13.0);
    assert_eq!(
        h.input_log().last().cloned(),
        Some(b"\x1b[200~arm -rf\x1b[201~".to_vec())
    );
}

// --- A15: focus-scoped routing --------------------------------------------------------------

#[test]
fn focused_terminal_encodes_keys_and_passes_super_shortcuts_to_the_application() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    assert!(h.update(|panel, window, cx| {
        use vespertine::Focusable as _;
        panel.focus_handle(cx).is_focused(window)
    }));
    h.host.simulate_keystrokes("cmd-k", None).expect("keys");
    assert_eq!(
        h.app_actions.get(),
        1,
        "the super-modified shortcut reaches the app"
    );
    h.advance(13.0);
    assert!(h.input_log().is_empty(), "and is not encoded");
    h.host.simulate_keystrokes("backspace", None).expect("keys");
    h.advance(13.0);
    assert_eq!(h.input_log(), vec![vec![0x7F]]);
    assert_eq!(
        h.text_actions.get(),
        0,
        "terminal input never reaches text actions"
    );

    // With focus elsewhere no key is encoded.
    let other = h.other_focus.clone();
    let window = h.host.window();
    h.host
        .app()
        .update_window(window, |_, window, cx| window.focus(&other, cx))
        .expect("window");
    h.host.flush();
    h.host.simulate_keystrokes("a b enter", None).expect("keys");
    h.advance(13.0);
    assert_eq!(h.input_log().len(), 1);
}

#[test]
fn clicking_focuses_and_a_pending_focus_request_applies_once_at_render() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    let other = h.other_focus.clone();
    let window = h.host.window();
    h.host
        .app()
        .update_window(window, |_, window, cx| window.focus(&other, cx))
        .expect("window");
    h.host.flush();
    assert!(!h.read(|panel| panel.has_pending_focus()));
    let (x, y) = h.cell_point(2, 2, 0.5);
    h.host.click(x, y, 0, "").expect("click");
    h.host.simulate_keystrokes("x", None).expect("keys");
    h.advance(13.0);
    assert_eq!(h.input_log(), vec![b"x".to_vec()]);

    // A cancelled request leaves focus alone; an uncancelled one claims it at the next render.
    h.update(|panel, _, cx| panel.request_focus(cx));
    h.update(|panel, _, _| panel.cancel_focus_request());
    assert!(!h.read(|panel| panel.has_pending_focus()));
}

// --- A5 geometry, A2 resize ----------------------------------------------------------------

#[test]
fn the_grid_follows_the_measured_bounds_and_the_size_event_is_debounced() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    let geometry = h.geometry();
    let padding = 12.0;
    let width = f32::from(geometry.bounds.size.width) - 2.0 * padding;
    let height = f32::from(geometry.bounds.size.height) - 2.0 * padding;
    assert_eq!(
        geometry.cols,
        (width / geometry.cell_width).floor() as usize
    );
    assert_eq!(
        geometry.rows,
        (height / geometry.line_height).floor() as usize
    );
    assert!((geometry.line_height - 18.0).abs() < 1e-3, "13 px x 18/13");
    let (cols, rows) = h.read(|panel| {
        let emulator = panel.active_emulator().expect("tab");
        (emulator.cols(), emulator.rows())
    });
    assert_eq!((cols, rows), (geometry.cols, geometry.rows));
    assert!(
        h.resizes.borrow().is_empty(),
        "debounced: nothing before 80 ms"
    );
    h.advance(79.0);
    assert!(h.resizes.borrow().is_empty());
    h.advance(2.0);
    assert_eq!(
        *h.resizes.borrow(),
        vec![(h.tab, geometry.cols, geometry.rows)]
    );
    h.advance(200.0);
    assert_eq!(
        h.resizes.borrow().len(),
        1,
        "an unchanged size is not reported again"
    );
}

// --- A9: selection --------------------------------------------------------------------------

#[test]
fn dragging_selects_text_and_a_click_selects_nothing() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    h.feed("hello world");
    let (x0, y) = h.cell_point(0, 0, 0.1);
    let (x1, _) = h.cell_point(0, 4, 0.9);
    h.press(x0, y, "");
    h.drag_to(x1, y);
    h.release(x1, y);
    assert_eq!(h.selection().as_deref(), Some("hello"));
    let selected = h.read(|panel| {
        panel
            .active_emulator()
            .expect("tab")
            .line(0)
            .iter()
            .take(6)
            .map(|cell| cell.selected)
            .collect::<Vec<_>>()
    });
    assert_eq!(selected, [true, true, true, true, true, false]);

    // A click without a drag leaves no selection.
    let (cx0, cy) = h.cell_point(0, 7, 0.5);
    h.press(cx0, cy, "");
    h.release(cx0, cy);
    assert_eq!(h.selection(), None);
    assert!(!h.read(|panel| panel.active_emulator().expect("tab").has_selection()));
}

#[test]
fn the_drag_threshold_is_two_pixels_and_anchors_at_the_press_cell() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    h.feed("abcdef");
    // Press just left of a cell's midpoint, move across it.
    let (x, y) = h.cell_point(0, 1, 0.45);
    h.press(x, y, "");
    h.drag_to(x + 1.9, y);
    assert!(!h.read(|panel| panel.active_emulator().expect("tab").has_selection()));
    h.release(x + 1.9, y);
    h.press(x, y, "");
    h.drag_to(x + 2.1, y);
    h.drag_to(x + 2.1 + 20.0, y);
    assert!(h.selection().is_some(), "2.1 px starts the selection");
    h.release(x + 22.1, y);
}

#[test]
fn double_and_triple_clicks_select_a_word_and_a_line() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    h.feed("alpha beta gamma\r\nsecond row");
    let (x, y) = h.cell_point(0, 7, 0.5);
    h.click_count(x, y, 2);
    assert_eq!(h.selection().as_deref(), Some("beta"));
    h.release(x, y);
    let (x, y) = h.cell_point(1, 3, 0.5);
    h.click_count(x, y, 3);
    assert_eq!(h.selection().as_deref(), Some("second row\n"));
    h.release(x, y);
}

#[test]
fn shift_click_extends_the_existing_selection() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    h.feed("hello world");
    let (x0, y) = h.cell_point(0, 0, 0.1);
    let (x1, _) = h.cell_point(0, 4, 0.9);
    h.press(x0, y, "");
    h.drag_to(x1, y);
    h.release(x1, y);
    let (x2, _) = h.cell_point(0, 9, 0.9);
    h.press(x2, y, "shift");
    h.release(x2, y);
    assert_eq!(h.selection().as_deref(), Some("hello worl"));
}

// --- A10: edge auto-scroll ------------------------------------------------------------------

#[test]
fn edge_scrolling_extends_the_selection_while_the_pointer_is_still() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    h.feed(&lines(80));
    let geometry = h.geometry();
    let (x, _) = h.cell_point(0, 2, 0.5);
    let (_, mid) = h.cell_point(geometry.rows / 2, 0, 0.0);
    // Start in the middle of the grid, drag above its top edge and keep the pointer there.
    h.press(x, mid, "");
    h.drag_to(x + 20.0, mid);
    let above = f64::from(f32::from(geometry.origin.y)) - 30.0;
    h.drag_to(x + 20.0, above);
    let first = h.selection().expect("selection").lines().count();
    assert_eq!(h.offset(), 0, "no tick yet");
    h.advance(24.0);
    let offset = h.offset();
    assert!(
        (1..=3).contains(&offset),
        "one tick scrolls 1 to 3 lines: {offset}"
    );
    let after_one = h.selection().expect("selection").lines().count();
    assert!(
        after_one > first,
        "the selection extends without pointer movement"
    );
    h.advance(24.0 * 5.0);
    assert!(
        h.offset() > offset,
        "the timer re-arms while the pointer stays in the band"
    );
    // Leaving the band stops scrolling; releasing cancels the timer.
    h.drag_to(x + 20.0, mid);
    let settled = h.offset();
    h.advance(24.0 * 4.0);
    assert_eq!(h.offset(), settled);
    h.release(x + 20.0, mid);
    h.advance(24.0 * 4.0);
    assert_eq!(h.offset(), settled);
}

// --- A11: wheel scrolling -------------------------------------------------------------------

fn wheel(h: &mut Harness, delta: ScrollDelta, phase: TouchPhase) {
    let event = ScrollWheelEvent {
        position: point(px(100.0), px(100.0)),
        delta,
        modifiers: Modifiers::none(),
        touch_phase: phase,
    };
    h.update(|panel, _, cx| panel.scroll_wheel(&event, cx));
}

#[test]
fn wheel_deltas_accumulate_per_tab_and_a_touch_start_resets() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    h.feed(&lines(80));
    let first = h.tab;
    let line_height = h.geometry().line_height;
    let quarter = ScrollDelta::Pixels(point(px(0.0), px(line_height / 4.0)));
    for _ in 0..3 {
        wheel(&mut h, quarter, TouchPhase::Moved);
    }
    assert_eq!(h.offset(), 0);
    wheel(&mut h, quarter, TouchPhase::Moved);
    assert_eq!(h.offset(), 1, "four quarter lines scroll one line");

    // Line deltas: 0.25 then 0.75 scrolls exactly one line on the second event.
    wheel(
        &mut h,
        ScrollDelta::Lines(point(0.0, 0.25)),
        TouchPhase::Moved,
    );
    assert_eq!(h.offset(), 1);
    wheel(
        &mut h,
        ScrollDelta::Lines(point(0.0, 0.75)),
        TouchPhase::Moved,
    );
    assert_eq!(h.offset(), 2);

    // A touch start resets the remainder.
    wheel(
        &mut h,
        ScrollDelta::Lines(point(0.0, 0.5)),
        TouchPhase::Moved,
    );
    wheel(
        &mut h,
        ScrollDelta::Lines(point(0.0, 0.5)),
        TouchPhase::Started,
    );
    assert_eq!(h.offset(), 2);

    // A second tab keeps its own remainder.
    h.update(|panel, _, cx| panel.select_tab(0, cx));
    let second = h.update(|panel, _, cx| panel.open_tab(cx));
    assert_ne!(first, second);
    let panel = h.panel.clone();
    h.host
        .app()
        .update(|cx| panel.update(cx, |panel, cx| panel.feed(second, lines(80).as_bytes(), cx)));
    wheel(
        &mut h,
        ScrollDelta::Lines(point(0.0, 0.5)),
        TouchPhase::Moved,
    );
    h.update(|panel, _, cx| panel.select_tab(0, cx));
    wheel(
        &mut h,
        ScrollDelta::Lines(point(0.0, 0.5)),
        TouchPhase::Moved,
    );
    assert_eq!(h.offset(), 3, "the first tab's own 0.5 + 0.5 made a line");
    h.update(|panel, _, cx| panel.select_tab(1, cx));
    wheel(
        &mut h,
        ScrollDelta::Lines(point(0.0, 0.5)),
        TouchPhase::Moved,
    );
    assert_eq!(
        h.offset(),
        1,
        "the second tab accumulated its own 0.5 + 0.5"
    );
}

#[test]
fn the_wheel_scrolls_the_view_through_real_event_dispatch_and_keys_snap_back() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    h.feed(&lines(80));
    let (x, y) = h.cell_point(3, 3, 0.5);
    let line_height = h.geometry().line_height;
    h.host
        .scroll_wheel(x, y, 0.0, f64::from(line_height) * 3.0, "")
        .expect("wheel");
    assert_eq!(h.offset(), 3);
    h.host.simulate_keystrokes("a", None).expect("keys");
    assert_eq!(
        h.offset(),
        0,
        "a keypress snaps the view to the live bottom"
    );
}

// --- Tabs -----------------------------------------------------------------------------------

#[test]
fn tabs_keep_their_own_emulators_and_closing_drops_their_state() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    let first = h.tab;
    h.feed("one");
    let second = h.update(|panel, _, cx| panel.open_tab(cx));
    let third = h.update(|panel, _, cx| panel.open_tab(cx));
    assert_eq!(
        h.read(|panel| panel.active_index()),
        2,
        "a new tab becomes active"
    );
    let panel = h.panel.clone();
    h.host.app().update(|cx| {
        panel.update(cx, |panel, cx| {
            panel.feed(second, b"two", cx);
            panel.feed(third, b"three", cx);
        })
    });
    h.host.flush();
    assert_eq!(
        h.read(|panel| panel.emulator(first).expect("tab").row_text(0)),
        "one"
    );
    assert_eq!(
        h.read(|panel| panel.emulator(second).expect("tab").row_text(0)),
        "two"
    );
    assert_eq!(h.text(0), "three");

    h.update(|panel, _, cx| panel.select_tab(0, cx));
    assert_eq!(
        h.text(0),
        "one",
        "switching only changes which emulator is painted"
    );

    h.update(|panel, _, cx| panel.move_tab(0, 2, cx));
    assert_eq!(h.read(|panel| panel.tab_ids()), vec![second, third, first]);
    assert_eq!(h.read(|panel| panel.active_index()), 2);
    let remaining = h.update(|panel, _, cx| panel.close_tab(second, cx));
    assert_eq!(remaining, 2);
    assert_eq!(h.read(|panel| panel.active_index()), 1);
    assert_eq!(h.read(|panel| panel.active_tab()), Some(first));
    // Closing a tab with queued input cancels its flush: nothing is written for it.
    h.update(|panel, _, cx| panel.send_input(third, b"x", cx));
    h.update(|panel, _, cx| panel.close_tab(third, cx));
    h.advance(30.0);
    assert!(h.input_log().is_empty());
    h.update(|panel, _, cx| panel.close_tab(first, cx));
    assert_eq!(h.read(|panel| panel.tab_count()), 0);
    assert_eq!(h.read(|panel| panel.active_index()), 0);
}

#[test]
fn title_bell_and_exit_lines_reach_the_host() {
    let Some(mut h) = harness(600.0, 400.0) else {
        return;
    };
    let titles: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
    let bells = Rc::new(Cell::new(0));
    let (titles_sink, bells_sink) = (titles.clone(), bells.clone());
    h.update(move |panel, _, _| {
        panel.on_title(move |_, title, _| titles_sink.borrow_mut().push(title.map(str::to_owned)));
        panel.on_bell(move |_, _| bells_sink.set(bells_sink.get() + 1));
    });
    h.feed("\x1b]0;build\x07\x07");
    h.feed("\x1b]2;\x07");
    assert_eq!(bells.get(), 1);
    assert_eq!(titles.borrow()[0].as_deref(), Some("build"));
    h.feed("done\r\n");
    let tab = h.tab;
    h.update(|panel, _, cx| panel.mark_exited(tab, 7, cx));
    h.feed(std::str::from_utf8(&exit_status_line(7)).expect("utf8"));
    assert!(h.text(2).contains("[process exited 7]"));
}

// --- A13: the dock --------------------------------------------------------------------------

struct DockRecord {
    layout: Option<(f32, f32)>,
    hitbox: Option<(f32, f32)>,
}

struct DockRoot {
    dock: Entity<BottomDock>,
}

impl Render for DockRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.dock.clone())
    }
}

#[test]
fn the_dock_clamps_its_height_and_the_content_above_tracks_it_in_the_same_frame() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let record = Rc::new(RefCell::new(DockRecord {
        layout: None,
        hitbox: None,
    }));
    let slot: Rc<RefCell<Option<(Entity<BottomDock>, Entity<TerminalPanel>)>>> = Rc::default();
    let mut host = TestHost::with_view(HostOptions::with_size(500.0, 600.0), {
        let record = record.clone();
        let slot = slot.clone();
        move |_window, cx| {
            let panel = cx.new(|cx| {
                let mut panel = TerminalPanel::new(cx);
                panel.set_theme(Some(test_theme()), cx);
                panel.open_tab(cx);
                panel
            });
            let dock = cx.new(|_| {
                BottomDock::new(panel.clone(), move |_, _| {
                    let record = record.clone();
                    let hitbox_record = record.clone();
                    canvas(
                        move |bounds, window, _| {
                            let hitbox =
                                window.insert_hitbox(bounds, vespertine::HitboxBehavior::Normal);
                            record.borrow_mut().layout =
                                Some((f32::from(bounds.size.width), f32::from(bounds.size.height)));
                            hitbox
                        },
                        move |_, hitbox, _, _| {
                            hitbox_record.borrow_mut().hitbox = Some((
                                f32::from(hitbox.bounds.size.width),
                                f32::from(hitbox.bounds.size.height),
                            ));
                        },
                    )
                    .size_full()
                    .into_any_element()
                })
            });
            *slot.borrow_mut() = Some((dock.clone(), panel));
            cx.new(|_| DockRoot { dock })
        }
    })
    .expect("host");
    let (dock, panel) = slot.borrow_mut().take().expect("dock");
    host.flush();

    let check = |host: &mut TestHost, request: f32, expected: f32| {
        host.app()
            .update(|cx| dock.update(cx, |dock, cx| dock.set_height(request, cx)));
        host.flush();
        host.flush();
        let geometry: DockGeometry = host
            .app()
            .update(|cx| dock.read(cx).geometry())
            .expect("geometry");
        assert_eq!(geometry.terminal_height, expected, "request {request}");
        assert_eq!(geometry.content_height, 600.0 - expected);
        let measured = host
            .app()
            .update(|cx| panel.read(cx).geometry())
            .expect("measured");
        assert!(
            (f32::from(measured.bounds.size.height) - (expected - TAB_HEIGHT)).abs() < 0.01,
            "the tab strip and the grid fill exactly the reserved height"
        );
        let record = record.borrow();
        assert_eq!(
            record.layout.map(|(_, h)| h),
            Some(600.0 - expected),
            "layout size of the content above"
        );
        assert_eq!(
            record.hitbox.map(|(_, h)| h),
            record.layout.map(|(_, h)| h),
            "hitbox size tracks the layout size in the same frame"
        );
    };
    check(&mut host, 10.0, 160.0);
    check(&mut host, f32::NAN, 160.0);
    check(&mut host, 300.0, 300.0);
    check(&mut host, 5000.0, 330.0);
    check(&mut host, f32::INFINITY, 160.0);
}

#[test]
fn short_windows_keep_the_minimum_dock_height() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let slot: Rc<RefCell<Option<Entity<BottomDock>>>> = Rc::default();
    let mut host = TestHost::with_view(HostOptions::with_size(400.0, 280.0), {
        let slot = slot.clone();
        move |_window, cx| {
            let panel = cx.new(|cx| {
                let mut panel = TerminalPanel::new(cx);
                panel.set_theme(Some(test_theme()), cx);
                panel.open_tab(cx);
                panel
            });
            let dock =
                cx.new(|_| BottomDock::new(panel, |_, _| div().size_full().into_any_element()));
            dock.update(cx, |_, _| {});
            *slot.borrow_mut() = Some(dock.clone());
            cx.new(|_| DockRoot { dock })
        }
    })
    .expect("host");
    let dock = slot.borrow_mut().take().expect("dock");
    host.app()
        .update(|cx| dock.update(cx, |dock, cx| dock.set_height(900.0, cx)));
    host.flush();
    let geometry = host
        .app()
        .update(|cx| dock.read(cx).geometry())
        .expect("geometry");
    assert_eq!(geometry.terminal_height, 160.0);
    assert_eq!(geometry.content_height, 120.0);
}

// --- A14: painted primitives ----------------------------------------------------------------

#[derive(Clone, Debug)]
struct PaintedQuad {
    order: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: Option<Hsla>,
    border: f32,
}

#[derive(Clone, Debug)]
struct PaintedGlyph {
    order: u32,
    x: f32,
    y: f32,
}

#[derive(Default)]
struct Captured {
    quads: Vec<PaintedQuad>,
    glyphs: Vec<PaintedGlyph>,
}

fn capturing_harness() -> Option<(Harness, Rc<RefCell<Captured>>)> {
    let captured: Rc<RefCell<Captured>> = Rc::default();
    let sink = captured.clone();
    let mut options = HostOptions::with_size(600.0, 400.0);
    options.scene_observer = Some(Box::new(move |scene| {
        let scale = DEFAULT_SCALE_FACTOR;
        let mut captured = sink.borrow_mut();
        captured.quads = scene
            .quads
            .iter()
            .map(|quad| PaintedQuad {
                order: quad.order,
                x: quad.bounds.origin.x.0 / scale,
                y: quad.bounds.origin.y.0 / scale,
                w: quad.bounds.size.width.0 / scale,
                h: quad.bounds.size.height.0 / scale,
                color: quad.background.as_solid(),
                border: quad.border_widths.top.0 / scale,
            })
            .collect();
        captured.glyphs = scene
            .monochrome_sprites
            .iter()
            .map(|sprite| (sprite.order, sprite.bounds))
            .chain(
                scene
                    .subpixel_sprites
                    .iter()
                    .map(|sprite| (sprite.order, sprite.bounds)),
            )
            .map(|(order, bounds)| PaintedGlyph {
                order,
                x: bounds.origin.x.0 / scale,
                y: bounds.origin.y.0 / scale,
            })
            .collect();
    }));
    harness_with(options).map(|h| (h, captured))
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.51
}

#[test]
fn the_cursor_is_a_translucent_block_when_focused_and_a_one_pixel_outline_when_not() {
    let Some((mut h, captured)) = capturing_harness() else {
        return;
    };
    h.feed("ab\x1b[1G");
    h.present();
    let geometry = h.geometry();
    let cell_x = f32::from(geometry.origin.x);
    let cell_y = f32::from(geometry.origin.y);
    let cursor = |captured: &Captured| {
        captured
            .quads
            .iter()
            .find(|quad| {
                near(quad.x, cell_x)
                    && near(quad.y, cell_y)
                    && near(quad.w, geometry.cell_width)
                    && near(quad.h, geometry.line_height)
            })
            .cloned()
    };
    let focused = cursor(&captured.borrow()).expect("cursor quad");
    let color = focused.color.expect("solid fill");
    assert!(
        color.a > 0.0 && color.a < 1.0,
        "translucent so the glyph stays legible"
    );
    assert_eq!(focused.border, 0.0);

    // Over a glyph the cursor is painted after it: the glyph stays visible beneath.
    let over_glyph = {
        let captured = captured.borrow();
        let cursor = captured
            .quads
            .iter()
            .find(|quad| {
                near(quad.x, f32::from(geometry.origin.x))
                    && near(quad.y, cell_y)
                    && near(quad.w, geometry.cell_width)
            })
            .expect("cursor over the first glyph");
        let glyph = captured
            .glyphs
            .iter()
            .find(|glyph| {
                near(glyph.x, f32::from(geometry.origin.x))
                    || glyph.x < f32::from(geometry.origin.x) + geometry.cell_width
            })
            .expect("glyph");
        (cursor.order, glyph.order)
    };
    assert!(
        over_glyph.0 > over_glyph.1,
        "cursor over glyph: {over_glyph:?}"
    );

    let other = h.other_focus.clone();
    let window = h.host.window();
    h.host
        .app()
        .update_window(window, |_, window, cx| window.focus(&other, cx))
        .expect("window");
    h.present();
    let unfocused = cursor(&captured.borrow()).expect("cursor outline");
    assert!(
        near(unfocused.border, 1.0),
        "a 1 px outline: {}",
        unfocused.border
    );
    assert!(
        unfocused.color.is_none_or(|color| color.a == 0.0),
        "no fill"
    );
}

#[test]
fn the_selection_wash_is_painted_between_backgrounds_and_glyphs() {
    let Some((mut h, captured)) = capturing_harness() else {
        return;
    };
    h.feed("\x1b[41mred\x1b[0m plain text");
    let (x0, y) = h.cell_point(0, 0, 0.1);
    let (x1, _) = h.cell_point(0, 7, 0.9);
    h.press(x0, y, "");
    h.drag_to(x1, y);
    h.release(x1, y);
    h.present();
    let geometry = h.geometry();
    let theme = test_theme();
    let captured = captured.borrow();
    let background = captured
        .quads
        .iter()
        .find(|quad| {
            near(quad.x, f32::from(geometry.origin.x))
                && near(quad.w, geometry.cell_width * 3.0)
                && near(quad.h, geometry.line_height)
        })
        .expect("a merged background run of the three red cells");
    assert_eq!(
        background.color.expect("solid").a,
        1.0,
        "explicit backgrounds are opaque"
    );
    let wash = captured
        .quads
        .iter()
        .find(|quad| {
            near(quad.x, f32::from(geometry.origin.x))
                && near(quad.w, geometry.cell_width * 8.0)
                && quad
                    .color
                    .is_some_and(|color| color.a > 0.0 && color.a < 0.5)
        })
        .expect("the selection wash as one merged run");
    assert!(wash.color.expect("solid").a < 0.5 && theme.selection.a < 0.5);
    // Glyphs of the first columns overlap both the background run and the wash.
    let first_glyph = captured
        .glyphs
        .iter()
        .filter(|glyph| {
            glyph.x >= f32::from(geometry.origin.x)
                && glyph.x < f32::from(geometry.origin.x) + geometry.cell_width * 3.0
                && glyph.y >= f32::from(geometry.origin.y) - 2.0
                && glyph.y < f32::from(geometry.origin.y) + geometry.line_height
        })
        .map(|glyph| glyph.order)
        .min()
        .expect("glyphs");
    assert!(
        background.order < wash.order,
        "wash after the background quads"
    );
    assert!(wash.order < first_glyph, "wash before the glyphs");
}

#[test]
fn wide_and_box_drawing_characters_do_not_shift_the_following_columns() {
    let Some((mut h, captured)) = capturing_harness() else {
        return;
    };
    h.feed("ab\u{2500}\u{4e16}cd\r\nabxxxcd");
    h.present();
    let geometry = h.geometry();
    let captured = captured.borrow();
    let row_y = |row: usize| f32::from(geometry.origin.y) + geometry.line_height * row as f32;
    let in_row = |row: usize| {
        let mut glyphs: Vec<&PaintedGlyph> = captured
            .glyphs
            .iter()
            .filter(|glyph| glyph.y >= row_y(row) - 2.0 && glyph.y < row_y(row + 1) - 2.0)
            .collect();
        glyphs.sort_by(|a, b| a.x.total_cmp(&b.x));
        glyphs
    };
    let first = in_row(0);
    let second = in_row(1);
    assert!(
        first.len() >= 6 && second.len() >= 7,
        "{} / {}",
        first.len(),
        second.len()
    );
    // 'c' and 'd' are the last two glyphs of both rows and start at the same columns.
    let (c1, d1) = (first[first.len() - 2].x, first[first.len() - 1].x);
    let (c2, d2) = (second[second.len() - 2].x, second[second.len() - 1].x);
    assert!(near(c1, c2), "c at column 5 in both rows: {c1} vs {c2}");
    assert!(near(d1, d2), "d at column 6 in both rows: {d1} vs {d2}");
    assert!(near(d1 - c1, geometry.cell_width), "one cell advance");
}

#[test]
fn segments_are_pinned_to_their_columns_and_ligatures_are_disabled() {
    let theme = test_theme();
    let font = terminal_font(&theme);
    let features = font.features.tag_value_list();
    for tag in ["liga", "calt", "dlig"] {
        assert!(
            features.contains(&(tag.to_string(), 0)),
            "{tag} is set to 0"
        );
    }
    assert_eq!(terminal_font_features().tag_value_list().len(), 3);
    let mut emulator = vespertine_components::terminal::Emulator::new(20, 2);
    emulator.feed("ab\u{2500}\u{4e16}cd e\u{301}".as_bytes());
    let cells = emulator.line(0);
    let segments = row_segments(&cells, &theme, &font);
    let described: Vec<(usize, &str)> = segments
        .iter()
        .map(|segment| (segment.col, segment.text.as_str()))
        .collect();
    assert_eq!(
        described,
        [
            (0, "ab"),
            (2, "\u{2500}"),
            (3, "\u{4e16}"),
            (5, "cd"),
            (8, "e\u{301}")
        ]
    );
}
